//! Untrusted documents are decoded with bounded output, time and concurrency.
use crate::application::{
    repositories::{AttachmentExtractor, PortFuture},
    usecases::attachments_usecase::{DOCX_CONTENT_TYPE, MAX_ATTACHMENT_TEXT_CHARS},
    utils::errors::ApplicationError,
};
use anyhow::Result;
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::Semaphore,
    time::timeout,
};

const EXTRACTION_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_TEXT_BYTES: usize = MAX_ATTACHMENT_TEXT_CHARS * 4;
const MAX_DOCX_XML_BYTES: usize = 4 * 1024 * 1024;
static EXTRACTION_SLOTS: Semaphore = Semaphore::const_new(2);
pub struct DocumentExtractor;

fn invalid(message: &'static str) -> anyhow::Error {
    ApplicationError::InvalidInput(message).into()
}

impl AttachmentExtractor for DocumentExtractor {
    fn extract<'a>(&'a self, content_type: &'a str, bytes: &'a [u8]) -> PortFuture<'a, String> {
        Box::pin(async move {
            let _permit = EXTRACTION_SLOTS
                .try_acquire()
                .map_err(|_| ApplicationError::ConversationBusy)?;
            match content_type {
                "text/plain" | "text/markdown" => {
                    if bytes.len() > MAX_TEXT_BYTES {
                        return Err(invalid("Text súboru prekračuje limit 100 000 znakov."));
                    }
                    String::from_utf8(bytes.to_vec())
                        .map_err(|_| invalid("Textové súbory musia používať kódovanie UTF-8."))
                }
                "application/pdf" => {
                    if !bytes.starts_with(b"%PDF-") {
                        return Err(invalid("Súbor nemá platnú hlavičku PDF."));
                    }
                    let mut command = Command::new("pdftotext");
                    command.args(["-layout", "-enc", "UTF-8", "-eol", "unix", "-", "-"]);
                    let output =
                        run_bounded(command, Some(bytes), MAX_TEXT_BYTES, EXTRACTION_TIMEOUT)
                            .await?;
                    String::from_utf8(output)
                        .map_err(|_| invalid("Text PDF sa nepodarilo prečítať v kódovaní UTF-8."))
                }
                DOCX_CONTENT_TYPE => {
                    if !bytes.starts_with(b"PK\x03\x04") {
                        return Err(invalid("Súbor nemá platný formát DOCX."));
                    }
                    let directory = DocumentDirectory::create()?;
                    let path = directory.0.join("document.docx");
                    tokio::fs::write(&path, bytes)
                        .await
                        .map_err(|_| anyhow::anyhow!("Pracovný súbor DOCX sa nepodarilo uložiť."))?;
                    let mut command = Command::new("unzip");
                    // Read only this member to stdout; no archive paths are ever extracted.
                    command.arg("-p").arg(&path).arg("word/document.xml");
                    let output =
                        run_bounded(command, None, MAX_DOCX_XML_BYTES, EXTRACTION_TIMEOUT).await?;
                    let xml = String::from_utf8(output)
                        .map_err(|_| invalid("Text dokumentu DOCX nepoužíva UTF-8."))?;
                    docx_text(&xml)
                }
                _ => Err(invalid("Nepodporovaný formát súboru.")),
            }
        })
    }
}

async fn run_bounded(
    mut command: Command,
    input: Option<&[u8]>,
    max_output: usize,
    limit: Duration,
) -> Result<Vec<u8>> {
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| {
            anyhow::anyhow!(
                "Extraktor súborov nie je dostupný. Server vyžaduje pdftotext a unzip."
            )
        })?;
    let stdin = child.stdin.take();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("Chýba výstup extraktora súborov."))?;
    timeout(limit, async {
        let write = async move {
            if let (Some(mut stdin), Some(bytes)) = (stdin, input) {
                stdin
                    .write_all(bytes)
                    .await
                    .map_err(|_| invalid("Súbor sa nepodarilo odovzdať extraktoru."))?;
                stdin
                    .shutdown()
                    .await
                    .map_err(|_| invalid("Súbor sa nepodarilo odovzdať extraktoru."))?;
            }
            Ok::<_, anyhow::Error>(())
        };
        let read = async move {
            let mut output = Vec::new();
            stdout
                .take((max_output + 1) as u64)
                .read_to_end(&mut output)
                .await
                .map_err(|_| invalid("Text súboru sa nepodarilo načítať."))?;
            if output.len() > max_output {
                return Err(invalid(
                    "Rozbalený dokument alebo jeho text prekračuje povolený limit.",
                ));
            }
            Ok::<_, anyhow::Error>(output)
        };
        let wait = async {
            if !child
                .wait()
                .await
                .map_err(|_| invalid("Spracovanie súboru zlyhalo."))?
                .success()
            {
                return Err(invalid(
                    "Súbor sa nepodarilo prečítať. Overte jeho formát a odstráňte prípadné heslo.",
                ));
            }
            Ok::<_, anyhow::Error>(())
        };
        let (_, output, _) = tokio::try_join!(write, read, wait)?;
        Ok(output)
    })
    .await
    .map_err(|_| invalid("Spracovanie súboru prekročilo časový limit 20 sekúnd."))?
}

struct DocumentDirectory(PathBuf);
impl DocumentDirectory {
    fn create() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("lesana-document-{}", uuid::Uuid::new_v4()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .map_err(|_| anyhow::anyhow!("Pracovný adresár DOCX sa nepodarilo vytvoriť."))?;
        Ok(Self(path))
    }
}
impl Drop for DocumentDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn docx_text(xml: &str) -> Result<String> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|_| invalid("Dokument DOCX obsahuje neplatné XML."))?;
    let ns = document.root_element().tag_name().namespace();
    if !matches!(
        ns,
        Some(
            "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
                | "http://purl.oclc.org/ooxml/wordprocessingml/main"
        )
    ) || document.root_element().tag_name().name() != "document"
    {
        return Err(invalid("Dokument nemá platnú štruktúru DOCX."));
    }
    let mut text = String::new();
    for node in document
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().namespace() == ns)
    {
        match node.tag_name().name() {
            "t" => text.push_str(node.text().unwrap_or_default()),
            "p" | "br" | "cr" | "tr" => text.push('\n'),
            "tab" => text.push('\t'),
            _ => (),
        }
        if text.len() > MAX_TEXT_BYTES {
            return Err(invalid("Text dokumentu prekračuje limit 100 000 znakov."));
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn docx_preserves_text_and_paragraphs_without_parsing_external_entities() {
        let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Zmluva &amp; príloha</w:t></w:r></w:p><w:p><w:r><w:t>Druhý odsek.</w:t></w:r></w:p></w:body></w:document>"#;
        assert_eq!(
            docx_text(xml).unwrap().trim(),
            "Zmluva & príloha\nDruhý odsek."
        );
        assert!(docx_text("<document>falošný</document>").is_err());
        assert!(
            docx_text("<!DOCTYPE document [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><document>&x;</document>")
                .is_err()
        );
    }
    #[tokio::test]
    async fn extraction_validates_signatures_and_encoding() {
        assert!(
            DocumentExtractor
                .extract("application/pdf", b"not pdf")
                .await
                .is_err()
        );
        assert!(
            DocumentExtractor
                .extract(DOCX_CONTENT_TYPE, b"not zip")
                .await
                .is_err()
        );
        assert!(
            DocumentExtractor
                .extract("text/plain", &[0xff])
                .await
                .is_err()
        );
        assert_eq!(
            DocumentExtractor
                .extract("text/plain", "Príloha".as_bytes())
                .await
                .unwrap(),
            "Príloha"
        );
    }
    #[tokio::test]
    async fn subprocess_output_and_runtime_are_bounded() {
        let mut command = Command::new("sh");
        command.args(["-c", "while :; do printf abcdefghijklmnop; done"]);
        assert!(
            run_bounded(command, None, 128, Duration::from_secs(2))
                .await
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
        let mut command = Command::new("sleep");
        command.arg("10");
        assert!(
            run_bounded(command, None, 128, Duration::from_millis(20))
                .await
                .unwrap_err()
                .to_string()
                .contains("časový")
        );
    }
}
