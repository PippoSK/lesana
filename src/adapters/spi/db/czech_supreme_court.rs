//! Official Czech Supreme Court HTML, retained in Czech as persuasive support.
use super::import::{Manifest, Node};
use crate::domain::Judgment;
use anyhow::{Context, Result, ensure};
use chrono::{Datelike, NaiveDate};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

const LEGAL_CONTEXT: &str = "Česká judikatúra · iba podporná argumentácia pre slovenský prípad. Rozhodnutie NS ČR nepreukazuje obsah slovenského práva ani záväzný výklad slovenského súdu. Pred použitím analógie porovnajte právnu úpravu, jej časové znenie a skutkové okolnosti s domácimi podkladmi. Text je v češtine; citácie ponechajte v origináli. Úryvky odôvodnenia sú interným členením textu, nie číslovanými bodmi súdu. Dátum rozhodnutia nie je účinnosť predpisu; overenie zdroja nepotvrdzuje neskorší vývoj judikatúry.";

fn re(value: &str) -> Regex {
    Regex::new(value).expect("static regex")
}
fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("static selector")
}
fn normalized(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn text(element: ElementRef<'_>) -> String {
    normalized(&element.text().collect::<String>())
}

fn decision_date(value: &str) -> Result<NaiveDate> {
    let compact = value.split_whitespace().collect::<String>();
    if let Ok(date) = NaiveDate::parse_from_str(&compact, "%d.%m.%Y") {
        return Ok(date);
    }
    let value = normalized(value).to_lowercase();
    let pattern = re(r"^([0-9]{1,2})\.\s+([\p{L}]+)\s+([0-9]{4})$");
    let parts = pattern
        .captures(&value)
        .context("Neplatný dátum rozhodnutia NS ČR.")?;
    let months = [
        "ledna",
        "února",
        "března",
        "dubna",
        "května",
        "června",
        "července",
        "srpna",
        "září",
        "října",
        "listopadu",
        "prosince",
    ];
    let month = months
        .iter()
        .position(|month| *month == &parts[2])
        .context("Neplatný mesiac rozhodnutia NS ČR.")?
        + 1;
    NaiveDate::from_ymd_opt(parts[3].parse()?, month as u32, parts[1].parse()?)
        .context("Neplatný dátum rozhodnutia NS ČR.")
}

// Inline markup does not create spaces inside words; block elements preserve paragraphs.
fn block_text(element: ElementRef<'_>, output: &mut String) {
    if matches!(element.value().name(), "script" | "style" | "noscript") {
        return;
    }
    let block = matches!(
        element.value().name(),
        "p" | "div" | "br" | "li" | "tr" | "h1" | "h2" | "h3"
    );
    if block {
        output.push('\n');
    }
    for child in element.children() {
        if let Some(value) = child.value().as_text() {
            output.push_str(value);
        } else if let Some(child) = ElementRef::wrap(child) {
            block_text(child, output);
        }
    }
    if block {
        output.push('\n');
    }
}
fn heading(word: &str) -> Regex {
    let spaced = word
        .chars()
        .map(|c| regex::escape(&c.to_string()))
        .collect::<Vec<_>>()
        .join(r"[\t ]*");
    re(&format!(r"(?im)^[\t ]*{spaced}[\t ]*:?[\t ]*$"))
}

struct Decision {
    judgment: Judgment,
    number: String,
    title: String,
    full_text: String,
    operative: String,
    reasoning: String,
    headnote: String,
}

fn read(html: &str, today: NaiveDate) -> Result<Decision> {
    ensure!(
        html.to_ascii_lowercase().contains("</html>"),
        "Neúplný HTML dokument NS ČR."
    );
    let document = Html::parse_document(html);
    let mut fields = HashMap::new();
    for row in document.select(&selector("#tabl > tbody > tr")) {
        let cells: Vec<_> = row
            .children()
            .filter_map(ElementRef::wrap)
            .filter(|e| e.value().name() == "td")
            .collect();
        if cells.len() != 2 {
            continue;
        }
        let label = text(cells[0]).trim_end_matches(':').trim().to_owned();
        ensure!(
            fields.insert(label, text(cells[1])).is_none(),
            "Duplicitné metadáta rozhodnutia NS ČR."
        );
    }
    let field = |label: &str| -> Result<&str> {
        fields
            .get(label)
            .map(String::as_str)
            .with_context(|| format!("Rozhodnutiu NS ČR chýba údaj {label}."))
    };
    ensure!(
        matches!(
            field("Soud")?,
            "Nejvyšší soud" | "Nejvyšší soud České republiky"
        ),
        "Dokument nie je rozhodnutím Najvyššieho súdu ČR."
    );
    let number = field("Spisová značka")?.to_owned();
    let compact = number.split_whitespace().collect::<String>().to_uppercase();
    let case_re = re(r"^([0-9]{1,3})([A-Z]{1,16})([0-9]{1,6})/([0-9]{4})$");
    let case = case_re
        .captures(&compact)
        .context("Nepodporovaná spisová značka NS ČR.")?;
    let ecli = field("ECLI")?.to_owned();
    let identity_re = re(
        r"^ECLI:CZ:NS:([0-9]{4}):([0-9]{1,3})\.([A-Z]{1,16})\.([0-9]{1,6})\.([0-9]{4})\.([1-9][0-9]*)$",
    );
    let identity = identity_re
        .captures(&ecli)
        .context("Chýba platné ECLI:CZ:NS rozhodnutia.")?;
    let decided_on = decision_date(field("Datum rozhodnutí")?)?;
    ensure!(
        decided_on <= today
            && decided_on.year() >= 1993
            && decided_on.year() == identity[1].parse::<i32>()?
            && (1..=4).all(|i| case[i] == identity[i + 1]),
        "Dátum, ECLI a spisová značka rozhodnutia NS ČR nesúhlasia."
    );
    let decision_type = match field("Typ rozhodnutí")? {
        "ROZSUDEK" => "Rozsudok",
        "USNESENÍ" => "Uznesenie",
        _ => anyhow::bail!("Podporované sú rozsudky a uznesenia NS ČR."),
    };
    let bodies: Vec<_> = document
        .select(&selector(".main_detail div[style]"))
        .filter(|e| {
            e.value()
                .attr("style")
                .unwrap_or("")
                .replace(' ', "")
                .eq_ignore_ascii_case("text-align:justify;")
        })
        .collect();
    ensure!(
        bodies.len() == 1,
        "Chýba jednoznačný text rozhodnutia NS ČR."
    );
    let mut raw = String::new();
    block_text(bodies[0], &mut raw);
    let full_text = raw
        .lines()
        .map(normalized)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let body_number = full_text
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .collect::<String>()
        .to_uppercase();
    ensure!(
        body_number == compact
            || body_number
                .strip_prefix(&compact)
                .is_some_and(|suffix| re(r"^-[0-9]+$").is_match(suffix)),
        "Spisová značka v texte nesúhlasí s metadátami."
    );
    ensure!(
        heading(field("Typ rozhodnutí")?).is_match(&full_text)
            && full_text.contains("Nejvyšší soud"),
        "Chýba hlavička rozhodnutia NS ČR."
    );
    let reasoning_heading = heading("Odůvodnění");
    let reasoning = reasoning_heading
        .find(&full_text)
        .context("Rozhodnutiu NS ČR chýba odôvodnenie.")?;
    let operative_re = re(r"(?i)\bt\s*a\s*k\s*t\s*o\s*:");
    let operative = operative_re
        .find(&full_text[..reasoning.start()])
        .context("Rozhodnutiu NS ČR chýba výrok.")?;
    // Older official decisions contain the notice without a "Poučení:" heading.
    // Still require an actual notice and a dated signature; a closed HTML wrapper alone
    // does not prove that the judgment text is complete.
    let appeal_notice = r"Proti tomuto (?:rozhodnutí|rozsudku|usnesení) není (?:přípustný opravný prostředek|opravný prostředek přípustný)\.";
    let instruction_re = re(&format!(
        r"(?im)^(?:P[\t ]*o[\t ]*u[\t ]*č[\t ]*e[\t ]*n[\t ]*í[\t ]*:[\t ]*(?:\n)?(?:{appeal_notice})?|{appeal_notice}[\t ]*$)"
    ));
    let instruction = instruction_re
        .find_iter(&full_text[reasoning.end()..])
        .last()
        .context("Rozhodnutiu NS ČR chýba záverečné poučenie; dokument môže byť neúplný.")?;
    let instruction_start = reasoning.end() + instruction.start();
    let ending = &full_text[instruction_start..];
    let signature_re = re(r"(?m)^V Brně dne ([^\n]+)$");
    let signature = signature_re
        .captures(ending)
        .context("Rozhodnutiu NS ČR chýba záverečný dátum.")?;
    ensure!(
        decision_date(&signature[1])? == decided_on
            && ending[signature.get(0).unwrap().end()..]
                .chars()
                .filter(|c| c.is_alphabetic())
                .count()
                >= 10,
        "Neúplný alebo nesúladný záver rozhodnutia NS ČR."
    );
    let operative = full_text[operative.end()..reasoning.start()]
        .trim()
        .to_owned();
    let reasoning = full_text[reasoning.end()..instruction_start]
        .trim()
        .to_owned();
    ensure!(
        operative.chars().count() > 10 && reasoning.chars().count() > 100,
        "Neúplný výrok alebo odôvodnenie NS ČR."
    );
    let headnote = fields.get("Právní věta").cloned().unwrap_or_default();
    Ok(Decision {
        title: format!("{decision_type} Najvyššieho súdu Českej republiky sp. zn. {number}"),
        judgment: Judgment {
            court: "Nejvyšší soud České republiky".into(),
            ecli,
            celex: String::new(),
            language: "CS".into(),
            decided_on,
            legal_context: LEGAL_CONTEXT.into(),
            supporting_only: true,
            aliases: vec![number.clone()],
            expected_points: 0,
            expected_pages: None,
            collection: None,
        },
        number,
        full_text,
        operative,
        reasoning,
        headnote,
    })
}

fn nodes(decision: &Decision) -> Vec<Node> {
    let node = |anchor: String, kind: &str, citation: &str, text: String, searchable: bool| Node {
        parent: (anchor != "judgment").then(|| "judgment".into()),
        anchor,
        kind: kind.into(),
        label: citation.into(),
        citation: citation.into(),
        heading: format!(
            "{} · {} · podporná argumentácia",
            decision.title, decision.judgment.ecli
        ),
        section: None,
        own_text: text.clone(),
        context_text: text.clone(),
        full_text: text,
        searchable,
        reference_links: vec![],
    };
    let mut nodes = vec![
        node(
            "judgment".into(),
            "judgment",
            "rozhodnutie (CS)",
            decision.full_text.clone(),
            false,
        ),
        node(
            "judgment-operative".into(),
            "judgment_operative",
            "výrok (CS)",
            decision.operative.clone(),
            true,
        ),
        node(
            "judgment-reasoning".into(),
            "judgment_reasoning",
            "odôvodnenie (CS)",
            decision.reasoning.clone(),
            false,
        ),
    ];
    // Preserve whole paragraphs and quoted lists; internal chunks are never called court points.
    let mut chunks = vec![String::new()];
    for paragraph in decision.reasoning.lines() {
        if chunks.last().unwrap().chars().count() >= 2400 {
            chunks.push(String::new());
        }
        let chunk = chunks.last_mut().unwrap();
        if !chunk.is_empty() {
            chunk.push('\n');
        }
        chunk.push_str(paragraph);
    }
    for (i, chunk) in chunks.into_iter().enumerate() {
        nodes.push(node(
            format!("judgment-reasoning-{}", i + 1),
            "judgment_reasoning",
            "odôvodnenie, nečíslovaný úryvok (CS)",
            chunk,
            true,
        ));
    }
    if !decision.headnote.is_empty() {
        nodes.push(node(
            "judgment-headnote".into(),
            "judgment_headnote",
            "právna veta (CS)",
            decision.headnote.clone(),
            true,
        ));
    }
    nodes
}

pub fn manifest(html: &str, source: &str, today: NaiveDate) -> Result<Manifest> {
    let decision = read(html, today)?;
    let count = nodes(&decision).len();
    let manifest = Manifest {
        act_id: decision
            .judgment
            .ecli
            .to_lowercase()
            .replace([':', '.'], "-"),
        valid_from: decision.judgment.decided_on,
        valid_to: None,
        verified_on: today,
        judgment: Some(decision.judgment),
        treaty: None,
        regulation: None,
        title: decision.title,
        number: decision.number,
        source_url: source.into(),
        pdf_url: String::new(),
        sha256: format!("{:x}", Sha256::digest(html.as_bytes())),
        expected_sections: 0,
        expected_letters: 0,
        expected_articles: 0,
        expected_paragraphs: Some(0),
        expected_nodes: Some(count),
    };
    super::import::parse(html, &manifest)?;
    Ok(manifest)
}

pub fn parse(html: &str, manifest: &Manifest) -> Result<Vec<Node>> {
    let decision = read(html, manifest.verified_on)?;
    let expected = manifest
        .judgment
        .as_ref()
        .context("Chýbajú metadáta NS ČR.")?;
    ensure!(
        serde_json::to_value(&decision.judgment)? == serde_json::to_value(expected)?
            && decision.title == manifest.title
            && decision.number == manifest.number
            && decision.judgment.decided_on == manifest.valid_from
            && manifest.valid_to.is_none(),
        "Metadáta nesúhlasia s oficiálnym rozhodnutím NS ČR."
    );
    Ok(nodes(&decision))
}

#[cfg(test)]
mod tests {
    use super::*;
    const HTML: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/nscr-33cdo314-2023.html"
    ));
    const ORDER: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/nscr-27cdo4352-2017.html"
    ));
    const OLDER: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/nscr-32cdo3337-2010.html"
    ));
    const SOURCE: &str = "https://rozhodnuti.nsoud.cz/Judikatura/judikatura_ns.nsf/WebSearch/A4D79508F15A7C04C1258A37004CF0D1?openDocument";
    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 13).unwrap()
    }

    #[test]
    fn czech_official_judgment_and_order_preserve_text_and_supporting_role() {
        for html in [HTML, ORDER] {
            let m = manifest(html, SOURCE, today()).unwrap();
            let j = m.judgment.as_ref().unwrap();
            assert!(j.supporting_only);
            assert_eq!(j.language, "CS");
            assert!(j.celex.is_empty() && m.pdf_url.is_empty());
            assert!(j.expected_pages.is_none() && j.expected_points == 0);
            let nodes = super::super::import::parse(html, &m).unwrap();
            assert!(nodes[0].full_text.contains("Poučení:"));
            assert!(nodes[0].full_text.contains("předseda senátu"));
            assert!(
                !nodes[0]
                    .full_text
                    .contains("Citace rozhodnutí Nejvyššího soudu by měla")
            );
            let operative = &nodes[1].full_text;
            assert!(operative.contains("se zamítá"));
            assert!(!operative.contains("Odůvodnění"));
            assert!(
                nodes
                    .iter()
                    .all(|n| n.kind != "judgment_point" && n.kind != "judgment_page")
            );
            let chunks = nodes
                .iter()
                .filter(|n| n.searchable && n.kind == "judgment_reasoning")
                .map(|n| n.full_text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(chunks, nodes[2].full_text);
            assert!(!chunks.contains("Poučení:"));
        }
    }

    #[test]
    fn czech_invalid_identity_incomplete_text_and_tampered_role_are_rejected() {
        for html in [
            HTML.replace(
                "ECLI:CZ:NS:2023:33.CDO.314.2023.1",
                "ECLI:CZ:NSS:2023:33.CDO.314.2023.1",
            ),
            HTML.replace(
                "ECLI:CZ:NS:2023:33.CDO.314.2023.1",
                "ECLI:CZ:NS:2022:33.CDO.314.2023.1",
            ),
            HTML.replace(
                "ECLI:CZ:NS:2023:33.CDO.314.2023.1",
                "ECLI:CZ:NS:2023:33.CDO.315.2023.1",
            ),
            HTML.replace("33 Cdo 314/2023-342", "33 Cdo 315/2023-342"),
            HTML.replace("O d ů v o d n ě n í</font>", "Chýbajúce odôvodnenie</font>"),
            HTML.replace("Poučení:", "Chýbajúce poučenie:"),
            HTML.replace("V Brně dne 17. 8. 2023", "V Brně dne 18. 8. 2023"),
            HTML.replace("</html>", ""),
            HTML.replace(">Nejvyšší soud</font>", ">Nejvyšší správní soud</font>"),
        ] {
            assert!(manifest(&html, SOURCE, today()).is_err());
        }
        assert!(manifest(HTML, SOURCE, NaiveDate::from_ymd_opt(2023, 8, 16).unwrap()).is_err());
        let mut m = manifest(HTML, SOURCE, today()).unwrap();
        m.judgment.as_mut().unwrap().supporting_only = false;
        assert!(super::super::import::parse(HTML, &m).is_err());
    }

    #[test]
    fn czech_headnote_is_separate_from_operative() {
        let html = HTML.replacen(
            "<img width=\"1\" height=\"1\" src=\"/icons/ecblank.gif\" border=\"0\" alt=\"\">",
            "Testovací právní věta.",
            1,
        );
        let m = manifest(&html, SOURCE, today()).unwrap();
        let nodes = parse(&html, &m).unwrap();
        assert_eq!(nodes.last().unwrap().anchor, "judgment-headnote");
        assert_eq!(nodes.last().unwrap().full_text, "Testovací právní věta.");
        assert!(!nodes[1].full_text.contains("Testovací"));
    }

    #[test]
    fn czech_older_judgment_without_notice_heading_preserves_complete_text() {
        let m = manifest(OLDER, SOURCE, today()).unwrap();
        assert_eq!(m.number, "32 Cdo 3337/2010");
        assert_eq!(m.valid_from, NaiveDate::from_ymd_opt(2012, 7, 24).unwrap());
        assert_eq!(
            m.judgment.as_ref().unwrap().ecli,
            "ECLI:CZ:NS:2012:32.CDO.3337.2010.1"
        );
        assert!(m.judgment.as_ref().unwrap().supporting_only);
        let nodes = super::super::import::parse(OLDER, &m).unwrap();
        assert!(
            nodes[0]
                .full_text
                .contains("Proti tomuto rozhodnutí není přípustný opravný prostředek.")
        );
        assert!(nodes[0].full_text.ends_with("předsedkyně senátu"));
        assert!(nodes[2].full_text.ends_with("náklady nevznikly."));
        assert!(!nodes[2].full_text.contains("Proti tomuto rozhodnutí"));
        assert!(!nodes[2].full_text.contains("V Brně dne"));
        assert_eq!(nodes.last().unwrap().anchor, "judgment-headnote");
    }

    #[test]
    fn czech_notice_layouts_keep_heading_and_notice_out_of_reasoning() {
        for label in ["P o u č e n í:", "POUČENÍ :", "Poučení:</font><br><font>"] {
            let html = HTML.replace("Poučení:", label);
            let m = manifest(&html, SOURCE, today()).unwrap();
            let nodes = parse(&html, &m).unwrap();
            assert!(!nodes[2].full_text.contains("Poučení"));
            assert!(!nodes[2].full_text.contains("P o u č e n í"));
            assert!(!nodes[2].full_text.contains("POUČENÍ"));
            assert!(!nodes[2].full_text.contains("Proti tomuto rozhodnutí"));
        }
    }

    #[test]
    fn czech_older_judgment_still_rejects_missing_notice_signature_and_wrong_dates() {
        let notice = "Proti tomuto rozhodnutí není přípustný opravný prostředek.";
        for html in [
            OLDER.replace(notice, ""),
            OLDER.replace("V Brně dne 24. července 2012", ""),
            OLDER.replace(
                "V Brně dne 24. července 2012",
                "V Brně dne 25. července 2012",
            ),
            OLDER.replace("V Brně dne 24. července 2012", "V Brně dne 31. února 2012"),
            OLDER
                .replace("JUDr. Hana  Gajdzioková", "")
                .replace("předsedkyně senátu", ""),
            OLDER.replace("</html>", ""),
        ] {
            assert!(manifest(&html, SOURCE, today()).is_err());
        }
        // A transport-complete wrapper must not make a truncated decision acceptable.
        let truncated = format!(
            "{}</div></div></body></html>",
            OLDER.split(notice).next().unwrap()
        );
        assert!(manifest(&truncated, SOURCE, today()).is_err());
    }

    #[test]
    fn czech_named_month_dates_are_validated() {
        for (i, month) in [
            "ledna",
            "února",
            "března",
            "dubna",
            "května",
            "června",
            "července",
            "srpna",
            "září",
            "října",
            "listopadu",
            "prosince",
        ]
        .iter()
        .enumerate()
        {
            assert_eq!(
                decision_date(&format!("24. {month} 2012")).unwrap(),
                NaiveDate::from_ymd_opt(2012, i as u32 + 1, 24).unwrap()
            );
        }
        assert!(decision_date("31. února 2012").is_err());
        assert!(decision_date("24. neznámý 2012").is_err());
    }
}
