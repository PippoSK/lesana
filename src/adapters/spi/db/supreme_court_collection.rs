//! Published Zbierka entries retain PDF pagination, headnotes and OCR provenance.
use super::import::{Manifest, Node};
use crate::domain::{Judgment, JudgmentCollection};
use anyhow::{Context, Result, ensure};
use chrono::{Datelike, NaiveDate};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Serialize, Deserialize)]
struct Snapshot {
    pdf_sha256: String,
    collection: JudgmentCollection,
    pages: Vec<String>,
}
fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static regular expression")
}
fn normalized(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
pub fn is_collection(pages: &[String]) -> bool {
    let front = pages
        .iter()
        .take(10)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    (front.contains("zbierka") || front.contains("zblerka"))
        && (front.contains("stanov") || front.contains("slanov"))
        && front.contains("rozhodnu")
        && (front.contains("najvyš") || front.contains("súdov"))
        && starts(pages).is_ok()
}
#[derive(Debug)]
struct Start {
    page: usize,
    number: u32,
    end: usize,
    statement: bool,
}
fn starts(pages: &[String]) -> Result<Vec<Start>> {
    let modern = re(r"(?im)^[\t ]*(ROZHODNUTIE|STANOVISKO)[\t ]+č\.[\t ]*([0-9]{1,3})[\t ]*\r?$");
    let legacy = re(r"(?im)^[\t ]*([0-9]{1,3})\.[\t ]*\r?\n\s*(ROZHODNUTIE|STANOVISKO)[\t ]*\r?$");
    let mut found = Vec::new();
    for (page, text) in pages.iter().enumerate() {
        let mut local = Vec::new();
        for c in modern.captures_iter(text) {
            local.push(Start {
                page,
                number: c[2].parse()?,
                end: c.get(0).unwrap().end(),
                statement: c[1].eq_ignore_ascii_case("STANOVISKO"),
            });
        }
        for c in legacy.captures_iter(text) {
            local.push(Start {
                page,
                number: c[1].parse()?,
                end: c.get(0).unwrap().end(),
                statement: c[2].eq_ignore_ascii_case("STANOVISKO"),
            });
        }
        ensure!(
            local.len() <= 1,
            "Viac záznamov na jednej strane zbierky zatiaľ nemožno bezpečne oddeliť."
        );
        found.extend(local);
    }
    ensure!(
        !found.is_empty(),
        "V zbierke sa nepodarilo rozpoznať začiatky rozhodnutí a stanovísk."
    );
    ensure!(
        found.windows(2).all(|pair| pair[0].number < pair[1].number),
        "Čísla záznamov zbierky nie sú jedinečné a vzostupné."
    );
    Ok(found)
}
fn issue(pages: &[String], today: NaiveDate) -> Result<(String, i32)> {
    let pattern =
        re(r"(?im)(?:zb[il]erka[^\n]*?|zo[šs][ií][tl]\s+)([1-9][0-9]?)\s*/\s*([0-9]{4})\b");
    let mut candidates = BTreeMap::new();
    for page in pages.iter().take(12) {
        for c in pattern.captures_iter(page) {
            let year = c[2].parse::<i32>()?;
            if (1950..=today.year()).contains(&year) {
                *candidates
                    .entry((format!("{}/{}", &c[1], year), year))
                    .or_insert(0usize) += 1;
            }
        }
    }
    candidates
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(identity, _)| identity)
        .context("Zbierke chýba jednoznačné označenie čiastky a roka.")
}
fn date(citation: &str, ocr: bool) -> Result<NaiveDate> {
    let numeric = re(r"\b([0-9]{1,2})\.\s*([0-9]{1,2})\.\s*([0-9]{4})\b");
    let named = re(r"(?i)\b([0-9]{1,2})\.\s*([a-záäčďéíľĺňóôŕšťúýž]+)\s+([0-9]{4})\b");
    let named_match = named.captures(citation);
    if let Some(c) = numeric.captures(citation) {
        if named_match
            .as_ref()
            .is_none_or(|n| c.get(0).unwrap().start() < n.get(0).unwrap().start())
        {
            return NaiveDate::from_ymd_opt(c[3].parse()?, c[2].parse()?, c[1].parse()?)
                .context("Neplatný dátum rozhodnutia v zbierke.");
        }
    }
    let c = named_match.context("Záznamu zbierky chýba dátum rozhodnutia.")?;
    let mut month = c[2].to_lowercase();
    // Restrict known OCR glyph substitutions to metadata; never rewrite source text.
    if ocr {
        month = match month.as_str() {
            "seplembra" | "sepiembra" => "septembra".into(),
            "oklóbra" | "oklobra" => "októbra".into(),
            _ => month,
        };
    }
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
    let index = months
        .iter()
        .position(|candidate| *candidate == month)
        .context("Neplatný mesiac rozhodnutia v zbierke.")?;
    NaiveDate::from_ymd_opt(c[3].parse()?, index as u32 + 1, c[1].parse()?)
        .context("Neplatný dátum rozhodnutia v zbierke.")
}
fn metadata(snapshot: &Snapshot, today: NaiveDate) -> Result<(Judgment, String, String, String)> {
    let collection = &snapshot.collection;
    ensure!(
        !snapshot.pages.is_empty()
            && collection.first_page > 0
            && collection.last_page == collection.first_page + snapshot.pages.len() - 1,
        "Neplatné strany záznamu zbierky."
    );
    ensure!(
        snapshot
            .pages
            .iter()
            .all(|p| p.chars().filter(|c| c.is_alphabetic()).count() >= 20),
        "Záznam zbierky obsahuje nečitateľnú stranu."
    );
    let found = starts(&snapshot.pages)?;
    ensure!(
        found.len() == 1 && found[0].page == 0 && found[0].number == collection.number,
        "Hranice záznamu nezodpovedajú zbierke."
    );
    let first = &found[0];
    let text = snapshot.pages.join("\n");
    let citation_re = re(r"(?is)\((?:uznesenie|rozsudok|stanovisko)\b[^)]{15,1600}\)");
    let citation = citation_re
        .find(&text[first.end..])
        .context("Záznamu zbierky chýba identifikácia pôvodného rozhodnutia.")?;
    let citation_start = first.end + citation.start();
    let citation_end = first.end + citation.end();
    let identity_limit: usize = snapshot
        .pages
        .iter()
        .take(2)
        .map(|page| page.len() + 1)
        .sum();
    ensure!(
        citation_start < identity_limit,
        "Identifikácia rozhodnutia nie je na úvodných stranách záznamu."
    );
    if let Some(reasoning) = re(r"(?im)^\s*Z\s+odôvodnenia\s*:").find(&text) {
        ensure!(
            citation_start < reasoning.start(),
            "Chýba úvodná identifikácia rozhodnutia pred odôvodnením."
        );
    }
    let identity = normalized(&text[citation_start..citation_end]);
    let court = identity.to_lowercase();
    ensure!(
        court.contains("najvyššieho súdu slovenskej republiky")
            || re(r"najvyššieho súdu sr\b").is_match(&court),
        "Záznam zbierky neidentifikuje Najvyšší súd Slovenskej republiky."
    );
    let case_re = re(
        r"(?i)sp\.[\t ]*zn\.[\t ]*((?:[0-9]{1,3}[\t ]*)?\p{L}[\p{L}\t ]{0,24}[\t ]*/?[\t ]*[0-9]{1,7}[\t ]*/[\t ]*[0-9]{2,4})\b",
    );
    let case = case_re
        .captures(&identity)
        .context("Záznamu zbierky chýba spisová značka.")?;
    let number = case[1].split_whitespace().collect::<String>();
    ensure!(
        re(r"^[\p{L}0-9]{1,24}/?[0-9]{1,7}/[0-9]{2,4}$").is_match(&number),
        "Spisová značka v zbierke nie je spoľahlivo čitateľná."
    );
    let decided_on = date(&identity, collection.ocr)?;
    ensure!(
        decided_on <= today && decided_on.year() <= collection.year,
        "Dátum rozhodnutia je v budúcnosti alebo po roku zbierky."
    );
    let headnote = normalized(&text[first.end..citation_start]);
    ensure!(
        headnote.chars().filter(|c| c.is_alphabetic()).count() >= 20,
        "Záznamu zbierky chýba právna veta."
    );
    ensure!(
        text[citation_end..]
            .chars()
            .filter(|c| c.is_alphabetic())
            .count()
            >= 30,
        "Záznam zbierky neobsahuje odôvodnenie alebo úplný publikovaný text."
    );
    let label = format!("R {}/{}", collection.number, collection.year);
    let kind = if first.statement {
        "Stanovisko"
    } else if identity.to_lowercase().starts_with("(uznesenie") {
        "Uznesenie"
    } else {
        "Rozsudok"
    };
    let title = format!("{label} · {kind} Najvyššieho súdu Slovenskej republiky sp. zn. {number}");
    let mut aliases = vec![number.clone(), label.clone()];
    // Preserve the raw OCR label and supply a known glyph-confusion lookup alias.
    if collection.ocr && re(r"^Ndi[0-9]+/[0-9]{2,4}$").is_match(&number) {
        aliases.push(number.replacen("Ndi", "Ndt", 1));
    }
    let mut legal_context = format!(
        "Zbierka stanovísk Najvyššieho súdu a rozhodnutí súdov SR, čiastka {}, {label}. Uložené je celé publikované znenie záznamu vrátane právnej vety; môže ísť o redakčný výňatok pôvodného rozhodnutia. Právna veta nie je výrok. Citácie označujú skutočné strany PDF zbierky. Dátum označuje rozhodnutie, nie vydanie zbierky.",
        collection.issue
    );
    if collection.ocr {
        legal_context.push_str(" Text bol rozpoznaný OCR zo skenu a môže obsahovať chyby; doslovné znenie overte v oficiálnom PDF.");
    }
    Ok((
        Judgment {
            supporting_only: false,
            court: "Najvyšší súd Slovenskej republiky".into(),
            ecli: String::new(),
            celex: String::new(),
            language: "SK".into(),
            decided_on,
            legal_context,
            aliases,
            expected_points: 0,
            expected_pages: Some(snapshot.pages.len()),
            collection: Some(collection.clone()),
        },
        number,
        title,
        headnote,
    ))
}
fn separator(page: &str) -> bool {
    let heading = re(
        r"^(?:rozhodn(?:utia|ulia|ulla) a stanoviská vo vec[il]ach(?: občianskoprávnych[i.,]* obchodných a správnych)?|občianskoprávnych, obchodných a správnych|zb[il]erka stanovísk ns sr a rozhodnutí súdov sr [0-9]{1,2}/[0-9]{4})$",
    );
    page.chars().filter(|c| c.is_alphabetic()).count() < 400
        && page.lines().all(|line| {
            let line = normalized(line).to_lowercase();
            !line.chars().any(|c| c.is_alphabetic()) || heading.is_match(&line)
        })
}
pub fn snapshot_manifests(
    pages: Vec<String>,
    pdf_sha256: String,
    source: &str,
    today: NaiveDate,
    ocr: bool,
) -> Result<Vec<(String, Manifest)>> {
    ensure!(
        pages.len() <= 500 && is_collection(&pages),
        "PDF nie je rozpoznanou čiastkou Zbierky stanovísk a rozhodnutí."
    );
    ensure!(
        re(r"^[a-f0-9]{64}$").is_match(&pdf_sha256),
        "Chýba kontrolný súčet oficiálneho PDF zbierky."
    );
    let entries = starts(&pages)?;
    let (issue, year) = issue(&pages, today)?;
    let front = pages[..entries[0].page].join("\n");
    let contents = re(r"(?im)^\s*OBSAH\s*$")
        .find(&front)
        .context("Zbierke chýba obsah na overenie úplnosti importu.")?;
    let toc = &front[contents.end()..];
    let numbers = re(r"(?m)^[\t ]*([0-9]{1,3})\.[\t ]*(?:[^\r\n]*)$");
    let expected: BTreeSet<u32> = numbers
        .captures_iter(toc)
        .map(|c| c[1].parse())
        .collect::<std::result::Result<_, _>>()?;
    let actual: BTreeSet<u32> = entries.iter().map(|entry| entry.number).collect();
    ensure!(
        !expected.is_empty() && expected == actual,
        "Počet alebo čísla záznamov nezodpovedajú obsahu zbierky; čiastočný import sa neuloží."
    );
    let appendix =
        re(r"(?i)\A\s*(?:[0-9]+\s*\n\s*)?(?:Uverejnené v Zbierke zák|V ZBIERKE ZÁKONOV|TIRÁŽ)");
    let end = pages
        .iter()
        .enumerate()
        .skip(entries.last().unwrap().page + 1)
        .find(|(_, page)| appendix.is_match(page))
        .map_or(pages.len(), |(index, _)| index);
    let mut documents = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let mut last = entries.get(index + 1).map_or(end, |next| next.page);
        while last > entry.page + 1 && separator(&pages[last - 1]) {
            last -= 1;
        }
        let snapshot = Snapshot {
            pdf_sha256: pdf_sha256.clone(),
            collection: JudgmentCollection {
                issue: issue.clone(),
                year,
                number: entry.number,
                first_page: entry.page + 1,
                last_page: last,
                ocr,
            },
            pages: pages[entry.page..last].to_vec(),
        };
        let (judgment, number, title, _) = metadata(&snapshot, today).with_context(|| {
            format!(
                "Záznam R {}/{year} sa nepodarilo bezpečne importovať",
                entry.number
            )
        })?;
        let snapshot = serde_json::to_string(&snapshot)?;
        let manifest = Manifest {
            act_id: format!("NSSR/ZBIERKA/{year}/{}", entry.number),
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
            expected_nodes: Some(last - entry.page + 2),
        };
        super::import::parse(&snapshot, &manifest)?;
        documents.push((snapshot, manifest));
    }
    Ok(documents)
}
pub fn parse(snapshot: &str, manifest: &Manifest) -> Result<Vec<Node>> {
    let snapshot: Snapshot =
        serde_json::from_str(snapshot).context("Neplatný snapshot záznamu zbierky.")?;
    ensure!(
        re(r"^[a-f0-9]{64}$").is_match(&snapshot.pdf_sha256),
        "Chýba kontrolný súčet oficiálneho PDF zbierky."
    );
    let (actual, number, title, headnote) = metadata(&snapshot, manifest.verified_on)?;
    let expected = manifest
        .judgment
        .as_ref()
        .context("Missing judgment metadata")?;
    ensure!(
        serde_json::to_value(&actual)? == serde_json::to_value(expected)?
            && manifest.number == number
            && manifest.title == title
            && manifest.valid_from == actual.decided_on
            && manifest.valid_to.is_none(),
        "Metadáta nesúhlasia s publikovaným záznamom zbierky."
    );
    let collection = &snapshot.collection;
    ensure!(
        manifest.act_id == format!("NSSR/ZBIERKA/{}/{}", collection.year, collection.number),
        "Identita záznamu nezodpovedá zbierke."
    );
    let full_text = snapshot
        .pages
        .iter()
        .map(|page| normalized(page))
        .collect::<Vec<_>>()
        .join("\n");
    let mut nodes = vec![Node {
        anchor: "judgment".into(),
        parent: None,
        kind: "judgment".into(),
        label: "publikované znenie".into(),
        citation: "publikované znenie v zbierke".into(),
        heading: title.clone(),
        section: None,
        own_text: String::new(),
        context_text: full_text.clone(),
        full_text,
        searchable: false,
        reference_links: vec![],
    }];
    for (offset, page) in snapshot.pages.iter().enumerate() {
        let number = collection.first_page + offset;
        let citation = format!("strana {number} PDF zbierky");
        let text = normalized(page);
        nodes.push(Node {
            anchor: format!("judgment-page-{number}"),
            parent: Some("judgment".into()),
            kind: "judgment_page".into(),
            label: citation.clone(),
            citation,
            heading: title.clone(),
            section: None,
            own_text: text.clone(),
            context_text: text.clone(),
            full_text: text,
            searchable: true,
            reference_links: vec![],
        });
    }
    nodes.push(Node {
        anchor: "judgment-headnote".into(),
        parent: Some("judgment".into()),
        kind: "judgment_headnote".into(),
        label: "právna veta".into(),
        citation: "právna veta".into(),
        heading: title,
        section: None,
        own_text: headnote.clone(),
        context_text: headnote.clone(),
        full_text: headnote,
        searchable: true,
        reference_links: vec![],
    });
    Ok(nodes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pages(text: &str) -> Vec<String> {
        text.trim_end_matches(['\n', '\r', '\u{c}'])
            .split('\u{c}')
            .map(|page| page.trim().to_owned())
            .collect()
    }
    fn make(pages: Vec<String>, ocr: bool) -> Result<Vec<(String, Manifest)>> {
        snapshot_manifests(
            pages,
            "a".repeat(64),
            "https://www.nsud.sk/data/att/fixture.pdf",
            NaiveDate::from_ymd_opt(2026, 10, 10).unwrap(),
            ocr,
        )
    }
    #[test]
    fn scanned_issue_preserves_all_entries_headnotes_and_original_pages() {
        let original = pages(include_str!(
            "../../../../tests/fixtures/nssr-zbierka-2002-2.txt"
        ));
        let documents = make(original.clone(), true).unwrap();
        assert_eq!(documents.len(), 16);
        let first_pages = [
            9, 10, 14, 17, 27, 31, 33, 36, 39, 43, 49, 55, 58, 63, 68, 72,
        ];
        let mut total = 0;
        for (index, (snapshot, manifest)) in documents.iter().enumerate() {
            let judgment = manifest.judgment.as_ref().unwrap();
            let collection = judgment.collection.as_ref().unwrap();
            assert_eq!(collection.issue, "2/2002");
            assert_eq!(collection.number, 20 + index as u32);
            assert_eq!(collection.first_page, first_pages[index]);
            assert!(collection.ocr && judgment.ecli.is_empty() && judgment.celex.is_empty());
            let nodes = super::super::import::parse(snapshot, manifest).unwrap();
            total += nodes.len();
            assert!(
                nodes
                    .iter()
                    .all(|node| node.kind != "judgment_operative" && node.kind != "judgment_point")
            );
            assert_eq!(nodes.last().unwrap().anchor, "judgment-headnote");
            for page in collection.first_page..=collection.last_page {
                let node = nodes
                    .iter()
                    .find(|node| node.anchor == format!("judgment-page-{page}"))
                    .unwrap();
                assert_eq!(node.full_text, normalized(&original[page - 1]));
            }
        }
        assert_eq!(total, 96);
        assert_eq!(
            documents[3]
                .1
                .judgment
                .as_ref()
                .unwrap()
                .collection
                .as_ref()
                .unwrap()
                .last_page,
            24
        );
        assert_eq!(
            documents[4].1.valid_from,
            NaiveDate::from_ymd_opt(2000, 9, 27).unwrap()
        );
        assert!(
            documents[2]
                .1
                .judgment
                .as_ref()
                .unwrap()
                .aliases
                .contains(&"Ndt122/99".into())
        );
        assert_eq!(
            documents
                .last()
                .unwrap()
                .1
                .judgment
                .as_ref()
                .unwrap()
                .collection
                .as_ref()
                .unwrap()
                .last_page,
            74
        );
        assert!(!documents.last().unwrap().0.contains("V ZBIERKE ZÁKONOV"));
    }
    #[test]
    fn official_modern_text_issue_keeps_case_date_and_pages() {
        let docs = make(
            pages(include_str!(
                "../../../../tests/fixtures/nssr-zbierka-2025-4.txt"
            )),
            false,
        )
        .unwrap();
        assert_eq!(docs.len(), 1);
        let m = &docs[0].1;
        assert_eq!(m.number, "1VObdo/1/2024");
        assert_eq!(m.valid_from, NaiveDate::from_ymd_opt(2025, 6, 26).unwrap());
        let c = m.judgment.as_ref().unwrap().collection.as_ref().unwrap();
        assert_eq!((c.number, c.first_page, c.last_page), (35, 3, 34));
        assert!(!c.ocr);
    }
    fn statement() -> Vec<String> {
        vec![
            "Zbierka stanovísk NS a rozhodnutí súdov SR 2/2025\nOBSAH\n3. Stanovisko kolégia".into(),
            "Zbierka stanovísk NS a rozhodnutí súdov SR 2/2025\nSTANOVISKO č. 3\nPrávna veta stanoviska k jednotnému výkladu ustanovenia zákona.\n(Stanovisko trestnoprávneho kolégia Najvyššieho súdu Slovenskej republiky zo 4. februára 2025, sp. zn. Tpj 1/2025)\nZ odôvodnenia:\nKolégium zistilo rozdielny výklad zákona a prijalo toto stanovisko na jeho zjednotenie.".into()
        ]
    }
    #[test]
    fn ordinary_body_lines_never_truncate_the_last_record() {
        let mut original = statement();
        original.push("Pokračovanie odôvodnenia stanoviska.\nV Zbierke zákonov bol tento právny predpis uverejnený už skôr.".into());
        original.push("Rozhodnutie odvolacieho súdu sa preto zrušuje.".into());
        let docs = make(original, false).unwrap();
        let (snapshot, manifest) = &docs[0];
        assert_eq!(
            manifest
                .judgment
                .as_ref()
                .unwrap()
                .collection
                .as_ref()
                .unwrap()
                .last_page,
            4
        );
        assert!(snapshot.contains("sa preto zrušuje"));
        assert!(snapshot.contains("bol tento právny predpis"));
    }
    #[test]
    fn statements_and_validation_reject_partial_wrong_and_tampered_entries() {
        let docs = make(statement(), false).unwrap();
        assert_eq!(docs[0].1.number, "Tpj1/2025");
        assert!(docs[0].1.title.contains("Stanovisko"));
        let mut missing = statement();
        missing[0].push_str("\n4. Chýbajúce rozhodnutie");
        assert!(make(missing, false).is_err());
        for (a, b) in [
            ("Slovenskej republiky", "Českej republiky"),
            ("4. februára 2025", "31. februára 2025"),
            ("4. februára 2025", "4. februára 2030"),
            ("(Stanovisko", "(Nečitateľné"),
        ] {
            let mut wrong = statement();
            wrong[1] = wrong[1].replace(a, b);
            assert!(make(wrong, false).is_err(), "{a} -> {b}");
        }
        let (snapshot, mut manifest) = docs.into_iter().next().unwrap();
        manifest
            .judgment
            .as_mut()
            .unwrap()
            .collection
            .as_mut()
            .unwrap()
            .first_page = 10;
        assert!(parse(&snapshot, &manifest).is_err());
        assert!(date("27. seplembra 2000", false).is_err());
        assert_eq!(
            date("z 31. októbra 2023, sťažnosť z 3.4.2024", false).unwrap(),
            NaiveDate::from_ymd_opt(2023, 10, 31).unwrap()
        );
    }
}
