use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::Semaphore,
    time::timeout,
};

const MAX_BYTES: usize = 100 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 20 * 1024 * 1024;
const MAX_PAGE_TEXT_BYTES: usize = 512 * 1024;
const MAX_PAGES: usize = 500;
const EXTRACTION_TIMEOUT: Duration = Duration::from_secs(30);
const OCR_TIMEOUT: Duration = Duration::from_secs(15 * 60);
static OCR_SLOTS: Semaphore = Semaphore::const_new(2);

/// The caller validates the official source URL and disables HTTP redirects.
pub async fn download_pages(
    client: &reqwest::Client,
    source: &str,
) -> Result<(Vec<String>, String, bool)> {
    let mut response = client
        .get(source)
        .header(reqwest::header::ACCEPT, "application/pdf")
        .send()
        .await
        .context("PDF rozhodnutia sa nepodarilo načítať.")?;
    ensure!(
        response.status().is_success(),
        "Oficiálne PDF nie je dostupné alebo vyžaduje nepodporované presmerovanie."
    );
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .unwrap_or("")
        .trim();
    ensure!(
        content_type.eq_ignore_ascii_case("application/pdf"),
        "Oficiálny zdroj neposkytol PDF dokument."
    );
    ensure!(
        response
            .content_length()
            .is_none_or(|n| n <= MAX_BYTES as u64),
        "PDF rozhodnutia je príliš veľké."
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("Sťahovanie PDF rozhodnutia zlyhalo.")?
    {
        ensure!(
            chunk.len() <= MAX_BYTES.saturating_sub(bytes.len()),
            "PDF rozhodnutia je príliš veľké."
        );
        bytes.extend_from_slice(&chunk);
    }
    validate_pdf(&bytes)?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let (pages, used_ocr) = extract_pages(&bytes).await?;
    Ok((pages, digest, used_ocr))
}

fn validate_pdf(bytes: &[u8]) -> Result<()> {
    ensure!(bytes.len() <= MAX_BYTES, "PDF rozhodnutia je príliš veľké.");
    ensure!(
        bytes.starts_with(b"%PDF-"),
        "Dokument nemá platnú hlavičku PDF."
    );
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(0, |i| i + 1);
    ensure!(
        bytes[..end].ends_with(b"%%EOF"),
        "PDF rozhodnutia je neúplné."
    );
    Ok(())
}

async fn extract_pages(bytes: &[u8]) -> Result<(Vec<String>, bool)> {
    let mut pages = extract_text_pages(bytes).await?;
    // Preserve the original validation path for native standalone decisions,
    // including a short final signature page. OCR is for collection publications.
    if pages.first().is_some_and(|page| {
        page.contains("Súd:") && page.contains("Spisová značka:") && page.contains("ECLI:")
    }) {
        return Ok((pages, false));
    }
    let used_ocr = pages.iter().any(needs_ocr);
    if used_ocr {
        timeout(OCR_TIMEOUT, ocr_pages(bytes, &mut pages))
            .await
            .context("Rozpoznávanie skenovaného PDF prekročilo časový limit.")??;
    }
    ensure!(
        pages
            .iter()
            .any(|page| page.chars().filter(|c| c.is_alphanumeric()).count() >= 80),
        "PDF neobsahuje dostatok čitateľného textu."
    );
    Ok((pages, used_ocr))
}

async fn extract_text_pages(bytes: &[u8]) -> Result<Vec<String>> {
    validate_pdf(bytes)?;
    let mut child = Command::new("pdftotext")
        // Layout keeps metadata labels and values together; formfeeds preserve pages.
        .args(["-layout", "-enc", "UTF-8", "-eol", "unix", "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("Extrakciu PDF sa nepodarilo spustiť. Server vyžaduje pdftotext z Poppleru.")?;
    let mut input = child.stdin.take().context("Chýba vstup extraktora PDF.")?;
    let output = child
        .stdout
        .take()
        .context("Chýba výstup extraktora PDF.")?;
    let operation = async {
        let write = async move {
            input
                .write_all(bytes)
                .await
                .context("PDF sa nepodarilo odovzdať extraktoru.")?;
            input
                .shutdown()
                .await
                .context("Vstup extraktora PDF sa nepodarilo ukončiť.")?;
            drop(input);
            Ok::<(), anyhow::Error>(())
        };
        let read = async move {
            let mut bounded = output.take((MAX_TEXT_BYTES + 1) as u64);
            let mut extracted = Vec::new();
            bounded
                .read_to_end(&mut extracted)
                .await
                .context("Text PDF sa nepodarilo načítať.")?;
            ensure!(
                extracted.len() <= MAX_TEXT_BYTES,
                "Extrahovaný text rozhodnutia je príliš veľký."
            );
            Ok::<Vec<u8>, anyhow::Error>(extracted)
        };
        let wait = async {
            let status = child
                .wait()
                .await
                .context("Extrakciu PDF sa nepodarilo dokončiť.")?;
            ensure!(
                status.success(),
                "PDF rozhodnutia sa nepodarilo spoľahlivo prečítať."
            );
            Ok::<(), anyhow::Error>(())
        };
        // Feed and drain concurrently so large documents cannot deadlock on pipes.
        let (_, extracted, _) = tokio::try_join!(write, read, wait)?;
        Ok::<Vec<u8>, anyhow::Error>(extracted)
    };
    let extracted = timeout(EXTRACTION_TIMEOUT, operation)
        .await
        .context("Extrakcia PDF prekročila časový limit.")??;
    let text = String::from_utf8(extracted).context("Text PDF nepoužíva UTF-8.")?;
    split_pages(&text)
}

fn needs_ocr(page: &String) -> bool {
    page.chars().filter(|c| c.is_alphanumeric()).count() < 80
}

struct OcrDirectory(PathBuf);
impl OcrDirectory {
    fn create() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("lesana-pdf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).context("Pracovný adresár OCR sa nepodarilo vytvoriť.")?;
        Ok(Self(path))
    }
}
impl Drop for OcrDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn ocr_pages(bytes: &[u8], pages: &mut [String]) -> Result<()> {
    let _permit = OCR_SLOTS.acquire().await.context("OCR nie je dostupné.")?;
    let directory = OcrDirectory::create()?;
    let source = directory.0.join("source.pdf");
    let output_prefix = directory.0.join("page");
    let image = directory.0.join("page.pgm");
    tokio::fs::write(&source, bytes)
        .await
        .context("Pracovné PDF pre OCR sa nepodarilo uložiť.")?;
    let mut total_bytes: usize = pages.iter().map(String::len).sum();
    for (index, page) in pages.iter_mut().enumerate() {
        if !needs_ocr(page) {
            continue;
        }
        let number = (index + 1).to_string();
        let mut render = Command::new("pdftoppm");
        render
            .args([
                "-f",
                &number,
                "-l",
                &number,
                "-scale-to",
                "2600",
                "-gray",
                "-singlefile",
            ])
            .arg(&source)
            .arg(&output_prefix);
        run_bounded(&mut render, EXTRACTION_TIMEOUT, 1024)
            .await
            .with_context(|| format!("Stranu {number} PDF sa nepodarilo pripraviť pre OCR."))?;
        ensure!(
            tokio::fs::metadata(&image).await?.len() <= 8 * 1024 * 1024,
            "Obrázok strany pre OCR je príliš veľký."
        );
        let mut recognize = Command::new("tesseract");
        recognize
            .arg(&image)
            .args(["stdout", "-l", "slk", "--psm", "3"])
            .env("OMP_THREAD_LIMIT", "1");
        let text = run_bounded(&mut recognize, Duration::from_secs(45), MAX_PAGE_TEXT_BYTES)
            .await
            .with_context(|| format!("Text strany {number} PDF sa nepodarilo rozpoznať."))?;
        let text = String::from_utf8(text).context("Rozpoznaný text PDF nepoužíva UTF-8.")?;
        let text = text.trim();
        if text.chars().filter(|c| c.is_alphanumeric()).count()
            > page.chars().filter(|c| c.is_alphanumeric()).count()
        {
            total_bytes = total_bytes - page.len() + text.len();
            ensure!(
                total_bytes <= MAX_TEXT_BYTES,
                "Rozpoznaný text PDF je príliš veľký."
            );
            *page = text.to_owned();
        }
        tokio::fs::remove_file(&image).await?;
    }
    Ok(())
}

async fn run_bounded(command: &mut Command, limit: Duration, max_output: usize) -> Result<Vec<u8>> {
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null())
        .kill_on_drop(true).spawn().context("Program OCR sa nepodarilo spustiť. Server vyžaduje Poppler a Tesseract so slovenčinou.")?;
    let stdout = child.stdout.take().context("Chýba výstup programu OCR.")?;
    timeout(limit, async {
        let read = async move {
            let mut bytes = Vec::new();
            stdout
                .take((max_output + 1) as u64)
                .read_to_end(&mut bytes)
                .await?;
            ensure!(
                bytes.len() <= max_output,
                "Výstup programu OCR je príliš veľký."
            );
            Ok::<_, anyhow::Error>(bytes)
        };
        let wait = async {
            ensure!(
                child.wait().await?.success(),
                "Program OCR nedokázal prečítať stranu PDF."
            );
            Ok::<_, anyhow::Error>(())
        };
        let (bytes, _) = tokio::try_join!(read, wait)?;
        Ok::<_, anyhow::Error>(bytes)
    })
    .await
    .context("Program OCR prekročil časový limit.")?
}

fn split_pages(text: &str) -> Result<Vec<String>> {
    let text = text.trim_end_matches([' ', '\t', '\r', '\n']);
    let text = text
        .strip_suffix('\u{000c}')
        .context("Extrakcia PDF neobsahuje úplné hranice strán.")?;
    let pages: Vec<_> = text
        .split('\u{000c}')
        .map(|p| p.trim().to_owned())
        .collect();
    ensure!(
        !pages.is_empty() && pages.len() <= MAX_PAGES,
        "PDF má nepodporovaný počet strán."
    );
    // Collections can have blank covers; the entry parser checks their body pages.
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn subprocess_output_failure_and_timeout_are_bounded() {
        let mut overflow = Command::new("sh");
        overflow.args(["-c", "while :; do printf abcdefghijklmnop; done"]);
        assert!(
            run_bounded(&mut overflow, Duration::from_secs(2), 128)
                .await
                .unwrap_err()
                .to_string()
                .contains("veľký")
        );
        let mut sleep = Command::new("sleep");
        sleep.arg("10");
        assert!(
            run_bounded(&mut sleep, Duration::from_millis(20), 128)
                .await
                .unwrap_err()
                .to_string()
                .contains("časový limit")
        );
        let mut failed = Command::new("false");
        assert!(
            run_bounded(&mut failed, Duration::from_secs(2), 128)
                .await
                .is_err()
        );
    }
    #[test]
    fn rejects_wrong_signature_and_truncated_pdf() {
        assert!(validate_pdf(b"<html>%%EOF").is_err());
        assert!(validate_pdf(b"%PDF-1.7\ntruncated").is_err());
        assert!(validate_pdf(b"%PDF-1.7\n%%EOF\ntrailing garbage").is_err());
        assert!(validate_pdf(b"%PDF-1.7\n%%EOF\r\n").is_ok());
    }
    #[test]
    fn preserves_page_boundaries_and_rejects_missing_text() {
        assert_eq!(
            split_pages("  prvá strana\n\u{000c}druhá strana\n\u{000c}").unwrap(),
            ["prvá strana", "druhá strana"]
        );
        assert!(split_pages("prvá strana").is_err());
        assert_eq!(
            split_pages("prvá\u{000c}\n \u{000c}").unwrap(),
            ["prvá", ""]
        );
        assert_eq!(split_pages("\u{000c}").unwrap(), [""]);
        assert!(split_pages(&"strana\u{000c}".repeat(MAX_PAGES + 1)).is_err());
    }
    #[tokio::test]
    async fn extracts_complete_official_supreme_court_pdf() {
        // https://www.nsud.sk/data/att/96d/805912.7b72da.pdf
        let pdf = include_bytes!("../../../../tests/fixtures/nssr-4ndc31-2024.pdf");
        assert_eq!(pdf.len(), 41_494);
        let (pages, used_ocr) = extract_pages(pdf).await.unwrap();
        assert!(!used_ocr);
        assert_eq!(pages.len(), 4);
        for expected in [
            "4Ndc/31/2024",
            "28.11.2024",
            "ECLI:SK:NSSR:2024:0021203612.1",
            "rozhodol:",
            "Odôvodnenie",
        ] {
            assert!(pages[0].contains(expected));
        }
        assert!(pages[3].contains("Poučenie:"));
        assert!(pages[3].contains("Proti tomuto uzneseniu"));
        assert_eq!(
            pages,
            split_pages(include_str!(
                "../../../../tests/fixtures/nssr-4ndc31-2024.txt"
            ))
            .unwrap()
        );
    }
}
