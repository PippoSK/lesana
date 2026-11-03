//! NSSR text PDFs retain their original page boundaries for honest citations.
use super::import::{Manifest, Node};
use crate::domain::Judgment;
use anyhow::{Context, Result, ensure};
use chrono::{Datelike, NaiveDate};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize)]
struct Snapshot {
    // Included in the snapshot checksum alongside extracted text and pagination.
    pdf_sha256: String,
    pages: Vec<String>,
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static regular expression")
}
fn normalized(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn field(header: &str, label: &str) -> Result<String> {
    let pattern = format!(r"(?m)^[\t ]*{}:[\t ]*\s*([^\r\n]+)", regex::escape(label));
    Ok(re(&pattern)
        .captures(header)
        .with_context(|| format!("Rozhodnutiu chýba údaj {label}."))?[1]
        .trim()
        .to_owned())
}
fn heading(word: &str) -> Regex {
    let spaced = word
        .chars()
        .map(|c| regex::escape(&c.to_string()))
        .collect::<Vec<_>>()
        .join(r"[\t ]*");
    re(&format!(
        r"(?im)^[\t ]*{spaced}(?:[\t ]*:[\t ]*|[\t ]*\r?$)"
    ))
}

fn decision_date(value: &str) -> Result<NaiveDate> {
    let compact = value.split_whitespace().collect::<String>();
    if let Ok(date) = NaiveDate::parse_from_str(&compact, "%d.%m.%Y") {
        return Ok(date);
    }
    let value = normalized(value).to_lowercase();
    let pattern = re(r"^([0-9]{1,2})\.\s+([a-záéíóúýôäčďľĺňŕšťž]+)\s+([0-9]{4})$");
    let c = pattern
        .captures(&value)
        .context("Neplatný dátum rozhodnutia.")?;
    let months = [
        "januára",
        "februára",
        "marca",
        "apríla",
        "mája",
        "júna",
        "júla",
        "augusta",
        "septembra",
        "októbra",
        "novembra",
        "decembra",
    ];
    let month = months
        .iter()
        .position(|m| *m == &c[2])
        .context("Neplatný mesiac rozhodnutia.")?
        + 1;
    NaiveDate::from_ymd_opt(c[3].parse()?, month as u32, c[1].parse()?)
        .context("Neplatný dátum rozhodnutia.")
}

fn metadata(pages: &[String], today: NaiveDate) -> Result<(Judgment, String, String, String)> {
    ensure!(
        !pages.is_empty()
            && pages.len() <= 500
            && pages
                .iter()
                .all(|p| p.chars().filter(|c| c.is_alphabetic()).count() >= 20),
        "PDF obsahuje prázdnu alebo nečitateľnú stranu; skenované rozhodnutia sa nepodporujú."
    );
    let header = &pages[0];
    let court = field(header, "Súd")?;
    ensure!(
        matches!(
            normalized(&court).as_str(),
            "Najvyšší súd SR" | "Najvyšší súd Slovenskej republiky"
        ),
        "Dokument nie je rozhodnutím Najvyššieho súdu SR."
    );
    let number = field(header, "Spisová značka")?
        .split_whitespace()
        .collect::<String>();
    ensure!(
        re(r"^[0-9]{1,3}\p{L}{1,16}/[0-9]{1,6}/[0-9]{4}$").is_match(&number),
        "Neplatná spisová značka rozhodnutia."
    );
    let ecli = field(header, "ECLI")?;
    let identity = re(r"^ECLI:SK:NSSR:([0-9]{4}):[A-Z0-9]+(?:\.[A-Z0-9]+)*$");
    let captured = identity
        .captures(&ecli)
        .context("Rozhodnutie nemá platné ECLI Najvyššieho súdu SR.")?;
    let decided_on = decision_date(&field(header, "Dátum vydania rozhodnutia")?)?;
    ensure!(
        decided_on <= today && decided_on.year() == captured[1].parse::<i32>()?,
        "Dátum rozhodnutia nesúhlasí s ECLI alebo je v budúcnosti."
    );
    let full = pages.join("\n");
    let decision_type = if heading("UZNESENIE").is_match(header) {
        "Uznesenie"
    } else if heading("ROZSUDOK").is_match(header) {
        "Rozsudok"
    } else {
        anyhow::bail!("Nepodporovaný typ rozhodnutia; vyžaduje sa rozsudok alebo uznesenie.")
    };
    let start = heading("rozhodol")
        .find(&full)
        .context("Rozhodnutiu chýba označenie výroku.")?;
    let reasoning = heading("Odôvodnenie")
        .find(&full)
        .context("Rozhodnutiu chýba odôvodnenie.")?;
    let instruction = heading("Poučenie")
        .find(&full)
        .context("Rozhodnutiu chýba záverečné poučenie; dokument môže byť neúplný.")?;
    ensure!(
        start.end() < reasoning.start()
            && reasoning.end() < instruction.start()
            && full[instruction.end()..].trim().chars().count() > 10,
        "Neúplná alebo nepodporovaná štruktúra rozhodnutia."
    );
    let operative = normalized(&full[start.end()..reasoning.start()]);
    ensure!(
        operative.chars().count() > 10,
        "Výrok rozhodnutia je neúplný."
    );
    let title = format!("{decision_type} Najvyššieho súdu Slovenskej republiky sp. zn. {number}");
    Ok((Judgment {
        supporting_only: false,
        court: "Najvyšší súd Slovenskej republiky".into(),
        ecli, celex: String::new(), language: "SK".into(), decided_on,
        legal_context: "Oficiálne rozhodnutie NS SR. Citácie označujú strany PDF; dátum rozhodnutia nie je dátum účinnosti právneho predpisu.".into(),
        aliases: vec![number.clone()], expected_points: 0, expected_pages: Some(pages.len()), collection: None,
    }, number, title, operative))
}

pub fn snapshot_manifest(
    pages: Vec<String>,
    pdf_sha256: String,
    source: &str,
    today: NaiveDate,
) -> Result<(String, Manifest)> {
    let (judgment, number, title, _) = metadata(&pages, today)?;
    let nodes = pages.len() + 2;
    let snapshot = serde_json::to_string(&Snapshot { pdf_sha256, pages })?;
    let manifest = Manifest {
        act_id: judgment.ecli.to_lowercase().replace([':', '.'], "-"),
        valid_from: judgment.decided_on,
        valid_to: None,
        verified_on: today,
        judgment: Some(judgment),
        treaty: None,
        regulation: None,
        title,
        number,
        source_url: source.into(),
        pdf_url: source.into(),
        sha256: format!("{:x}", Sha256::digest(snapshot.as_bytes())),
        expected_sections: 0,
        expected_letters: 0,
        expected_articles: 0,
        expected_paragraphs: Some(0),
        expected_nodes: Some(nodes),
    };
    super::import::parse(&snapshot, &manifest)?;
    Ok((snapshot, manifest))
}

pub fn parse(snapshot: &str, manifest: &Manifest) -> Result<Vec<Node>> {
    let snapshot: Snapshot =
        serde_json::from_str(snapshot).context("Neplatný snapshot rozhodnutia NS SR.")?;
    ensure!(
        re(r"^[a-f0-9]{64}$").is_match(&snapshot.pdf_sha256),
        "Chýba kontrolný súčet oficiálneho PDF."
    );
    let (actual, number, title, operative) = metadata(&snapshot.pages, manifest.verified_on)?;
    let expected = manifest
        .judgment
        .as_ref()
        .context("Missing judgment metadata")?;
    ensure!(
        actual.ecli == expected.ecli
            && actual.decided_on == expected.decided_on
            && actual.court == expected.court
            && expected.celex.is_empty()
            && actual.expected_pages == expected.expected_pages
            && expected.expected_points == 0
            && number == manifest.number
            && title == manifest.title
            && manifest.valid_from == actual.decided_on
            && manifest.valid_to.is_none(),
        "Metadáta nesúhlasia s oficiálnym rozhodnutím."
    );
    let full_text = snapshot
        .pages
        .iter()
        .map(|p| normalized(p))
        .collect::<Vec<_>>()
        .join("\n");
    let heading = format!("{} · {}", manifest.title, actual.ecli);
    let mut nodes = vec![Node {
        anchor: "judgment".into(),
        parent: None,
        kind: "judgment".into(),
        label: "rozhodnutie".into(),
        citation: "rozhodnutie".into(),
        heading: heading.clone(),
        section: None,
        own_text: String::new(),
        context_text: full_text.clone(),
        full_text,
        searchable: false,
        reference_links: vec![],
    }];
    for (index, page) in snapshot.pages.iter().enumerate() {
        let page = normalized(page);
        let citation = format!("strana {} PDF", index + 1);
        nodes.push(Node {
            anchor: format!("judgment-page-{}", index + 1),
            parent: Some("judgment".into()),
            kind: "judgment_page".into(),
            label: citation.clone(),
            citation,
            heading: heading.clone(),
            section: None,
            own_text: page.clone(),
            context_text: page.clone(),
            full_text: page,
            searchable: true,
            reference_links: vec![],
        });
    }
    nodes.push(Node {
        anchor: "judgment-operative".into(),
        parent: Some("judgment".into()),
        kind: "judgment_operative".into(),
        label: "výrok".into(),
        citation: "výrok".into(),
        heading: "Výrok".into(),
        section: None,
        own_text: operative.clone(),
        context_text: operative.clone(),
        full_text: operative,
        searchable: true,
        reference_links: vec![],
    });
    Ok(nodes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Vec<String> {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/nssr-4ndc31-2024.txt"
        ))
        .trim_end_matches(['\n', '\r', '\u{c}'])
        .split('\u{c}')
        .map(|p| p.trim().to_owned())
        .collect()
    }
    const SOURCE: &str = "https://www.nsud.sk/data/att/96d/805912.7b72da.pdf";
    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 4).unwrap()
    }
    fn snapshot(pages: Vec<String>) -> Result<(String, Manifest)> {
        snapshot_manifest(
            pages,
            format!(
                "{:x}",
                Sha256::digest(include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/nssr-4ndc31-2024.pdf"
                )))
            ),
            SOURCE,
            today(),
        )
    }
    #[test]
    fn preserves_real_pdf_pages_identity_operative_and_complete_text() {
        let (text, manifest) = snapshot(fixture()).unwrap();
        assert_eq!(manifest.number, "4Ndc/31/2024");
        assert_eq!(
            manifest.judgment.as_ref().unwrap().ecli,
            "ECLI:SK:NSSR:2024:0021203612.1"
        );
        assert_eq!(manifest.judgment.as_ref().unwrap().expected_pages, Some(4));
        let nodes = super::super::import::parse(&text, &manifest).unwrap();
        assert_eq!(nodes.len(), 6);
        for (index, page) in fixture().iter().enumerate() {
            assert_eq!(nodes[index + 1].full_text, normalized(page));
            assert!(nodes[0].full_text.contains(&nodes[index + 1].full_text));
        }
        assert!(nodes.iter().all(|n| n.kind != "judgment_point"));
        assert!(!nodes[5].full_text.contains("Odôvodnenie"));
        assert!(!nodes[5].full_text.contains("Poučenie"));
        assert!(nodes[0].full_text.contains("Poučenie"));
        assert!(super::super::import::parse(&(text + " "), &manifest).is_err());
    }
    #[test]
    fn accepts_spaced_headings_and_inline_instruction_without_losing_text() {
        let pages = fixture()
            .iter()
            .map(|p| {
                p.replace("rozhodol:", "r o z h o d o l :")
                    .replace("Odôvodnenie", "O d ô v o d n e n i e")
                    .replace("Poučenie:\n", "Poučenie: ")
            })
            .collect();
        let (text, manifest) = snapshot(pages).unwrap();
        let nodes = super::super::import::parse(&text, &manifest).unwrap();
        assert!(nodes[0].full_text.contains("Proti tomuto uzneseniu"));
        assert!(nodes[5].full_text.contains("Na prejednanie sporu"));
    }
    #[test]
    fn older_pdf_with_named_month_keeps_all_seven_pages() {
        let text = include_str!("../../../../tests/fixtures/nssr-5cdo62-2018.txt");
        let pages: Vec<_> = text
            .trim_end_matches(['\n', '\r', '\u{c}'])
            .split('\u{c}')
            .map(|p| p.trim().to_owned())
            .collect();
        let pdf = include_bytes!("../../../../tests/fixtures/nssr-5cdo62-2018.pdf");
        let (snapshot, manifest) = snapshot_manifest(
            pages,
            format!("{:x}", Sha256::digest(pdf)),
            "https://www.nsud.sk/data/att/66965.pdf",
            today(),
        )
        .unwrap();
        assert_eq!(manifest.number, "5Cdo/62/2018");
        assert_eq!(
            manifest.valid_from,
            NaiveDate::from_ymd_opt(2019, 12, 11).unwrap()
        );
        assert_eq!(manifest.judgment.as_ref().unwrap().expected_pages, Some(7));
        let nodes = super::super::import::parse(&snapshot, &manifest).unwrap();
        assert_eq!(nodes.len(), 9);
        assert!(nodes.last().unwrap().full_text.contains("Z r u š u j e"));
        for (i, month) in [
            "januára",
            "februára",
            "marca",
            "apríla",
            "mája",
            "júna",
            "júla",
            "augusta",
            "septembra",
            "októbra",
            "novembra",
            "decembra",
        ]
        .iter()
        .enumerate()
        {
            assert_eq!(
                decision_date(&format!("1. {month} 2024")).unwrap(),
                NaiveDate::from_ymd_opt(2024, i as u32 + 1, 1).unwrap()
            );
        }
        assert!(decision_date("31. februára 2024").is_err());
        assert!(decision_date("11. december 2019").is_err());
    }
    #[test]
    fn rejects_wrong_court_identity_dates_missing_pages_and_incomplete_text() {
        let pages = fixture();
        for (old, new) in [
            ("Najvyšší súd SR", "Najvyšší správny súd SR"),
            ("ECLI:SK:NSSR:", "ECLI:SK:NSSSR:"),
            ("28.11.2024", "28.11.2034"),
            ("Poučenie", "Chýba koniec"),
            ("Odôvodnenie", "Chýba odôvodnenie"),
        ] {
            let corrupt: Vec<_> = pages.iter().map(|p| p.replace(old, new)).collect();
            assert_ne!(corrupt, pages, "fixture must contain {old}");
            assert!(snapshot(corrupt).is_err(), "{old}");
        }
        let mut missing = pages.clone();
        missing.pop();
        assert!(snapshot(missing).is_err());
        let mut scanned = pages;
        scanned[1] = String::new();
        assert!(snapshot(scanned).is_err());
        let (text, mut manifest) = snapshot(fixture()).unwrap();
        manifest.judgment.as_mut().unwrap().ecli.push('0');
        assert!(super::super::import::parse(&text, &manifest).is_err());
    }
    #[sqlx::test(migrations = "./migrations")]
    #[ignore = "requires disposable PostgreSQL via DATABASE_URL"]
    async fn import_and_exact_search_preserve_pages_and_judgment_context(pool: sqlx::PgPool) {
        let (text, manifest) = snapshot(fixture()).unwrap();
        super::super::import::import(&pool, &text, &manifest)
            .await
            .unwrap();
        for query in [
            "4Ndc/31/2024 strana 2",
            "ECLI:SK:NSSR:2024:0021203612.1 strana 2",
        ] {
            let found = super::super::search::search(&pool, None, query, today(), 6)
                .await
                .unwrap();
            assert_eq!(found.len(), 1, "{query}");
            assert_eq!(found[0].kind, "judgment_page");
            assert_eq!(found[0].text, normalized(&fixture()[1]));
        }
        let found = super::super::search::get_judgment(&pool, "4Ndc/31/2024", today())
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].text.contains("Poučenie"));
        let first = super::super::search::search(&pool, None, "4Ndc/31/2024 výrok", today(), 6)
            .await
            .unwrap();
        super::super::import::import(&pool, &text, &manifest)
            .await
            .unwrap();
        let again = super::super::search::search(&pool, None, "4Ndc/31/2024 výrok", today(), 6)
            .await
            .unwrap();
        assert_eq!(first[0].id, again[0].id);
        assert!(
            super::super::search::search(
                &pool,
                None,
                "ECLI:SK:NSSR:2024:0021203612.10",
                today(),
                6
            )
            .await
            .unwrap()
            .is_empty()
        );
    }
}
