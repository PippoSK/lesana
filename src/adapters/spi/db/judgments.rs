use crate::adapters::spi::db::import::{Manifest, Node};
use anyhow::{Context, Result, ensure};
use regex::Regex;
use scraper::{Html, Selector};
use sqlx::PgPool;

pub use crate::domain::Judgment;

// EUR-Lex's older CJEU HTML has numbered paragraphs with continuation blocks,
// but no paragraph anchors. Internal anchors must never be appended to its URL.
pub fn parse(html: &str, manifest: &Manifest) -> Result<Vec<Node>> {
    let metadata = manifest
        .judgment
        .as_ref()
        .context("Missing judgment metadata")?;
    ensure!(
        metadata.decided_on == manifest.valid_from && manifest.valid_to.is_none(),
        "Invalid judgment date"
    );
    ensure!(
        metadata.expected_points > 0 && !metadata.ecli.is_empty(),
        "Incomplete judgment metadata"
    );
    let document = Html::parse_document(html);
    if document
        .select(&Selector::parse("body > p.sum-title-1").unwrap())
        .next()
        .is_some()
    {
        return parse_modern(&document, manifest);
    }
    let mut nodes = Vec::<Node>::new();
    let mut complete = Vec::new();
    let mut heading = String::new();
    let mut started = false;
    let mut in_operative = false;
    let mut points = 0;
    for p in document.select(&Selector::parse("body > p").unwrap()) {
        let class = p.value().attr("class").unwrap_or("");
        if class == "C19Centre" && !started {
            started = true;
        }
        if !started {
            continue;
        } // The preceding abstract is not the judgment.
        let text = p
            .text()
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            continue;
        }
        complete.push(text.clone());
        if class == "C04Titre1" {
            heading = text;
            continue;
        }
        if class == "C01PointnumeroteAltN" {
            points += 1;
            let number: usize = text
                .split_whitespace()
                .next()
                .context("Missing point number")?
                .parse()?;
            ensure!(
                number == points,
                "Missing, duplicate or out-of-order judgment point"
            );
            nodes.push(node(
                format!("judgment-point-{number}"),
                "judgment_point",
                format!("bod {number}"),
                &heading,
                text,
            ));
        } else if class == "C41DispositifIntroduction" {
            ensure!(!in_operative, "Duplicate operative part");
            in_operative = true;
            nodes.push(node(
                "judgment-operative".into(),
                "judgment_operative",
                "výrok".into(),
                "Výrok",
                text,
            ));
        } else if class == "C77Signatures" || class == "C42FootnoteLangue" {
            continue;
        } else if let Some(last) = nodes.last_mut() {
            // Quoted legislation, questions, and bullets belong to the current
            // numbered point; never drop its continuation or create fake points.
            last.own_text.push('\n');
            last.own_text.push_str(&text);
            last.full_text = last.own_text.clone();
            last.context_text = last.own_text.clone();
        }
    }
    ensure!(
        points == metadata.expected_points && in_operative,
        "Incomplete judgment or missing operative part"
    );
    let full_text = complete.join("\n");
    nodes.insert(
        0,
        Node {
            anchor: "judgment".into(),
            parent: None,
            kind: "judgment".into(),
            label: "rozsudok".into(),
            heading: format!(
                "{} · {} · {}",
                manifest.title, metadata.ecli, metadata.legal_context
            ),
            citation: "rozsudok".into(),
            section: None,
            own_text: String::new(),
            context_text: full_text.clone(),
            full_text,
            searchable: false,
            reference_links: vec![],
        },
    );
    Ok(nodes)
}

fn node(anchor: String, kind: &str, citation: String, heading: &str, text: String) -> Node {
    Node {
        anchor,
        parent: Some("judgment".into()),
        kind: kind.into(),
        label: citation.clone(),
        citation,
        heading: heading.into(),
        section: None,
        own_text: text.clone(),
        full_text: text.clone(),
        context_text: text,
        searchable: true,
        reference_links: vec![],
    }
}

// Cellar's XHTML uses top-level tables for numbered paragraphs, with nested
// tables for quoted lists. Keep each whole table with its point, including lists.
fn parse_modern(document: &Html, manifest: &Manifest) -> Result<Vec<Node>> {
    let metadata = manifest
        .judgment
        .as_ref()
        .context("Missing judgment metadata")?;
    let body = document
        .select(&Selector::parse("body").unwrap())
        .next()
        .context("Missing body")?;
    let count_selector = Selector::parse("p.count").unwrap();
    let signature_selector = Selector::parse(".signaturecase").unwrap();
    let mut nodes = Vec::<Node>::new();
    let mut complete = Vec::new();
    let mut heading = String::new();
    let mut points = 0;
    let mut operative = false;
    let mut operative_body = false;
    let mut ended = false;
    for block in body.children().filter_map(scraper::ElementRef::wrap) {
        let text = block
            .text()
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            continue;
        }
        complete.push(text.clone());
        if block.select(&signature_selector).next().is_some()
            || block.value().attr("class") == Some("note")
        {
            ended = true;
        }
        if ended {
            continue;
        }
        let class = block.value().attr("class").unwrap_or("");
        if class.starts_with("title-grseq-") {
            heading = text;
            continue;
        }
        if text.starts_with("Z týchto dôvodov") && text.contains("rozhodol") {
            ensure!(!operative && points > 0, "Invalid operative part");
            operative = true;
            nodes.push(node(
                "judgment-operative".into(),
                "judgment_operative",
                "výrok".into(),
                "Výrok",
                text,
            ));
            continue;
        }
        let number = (!operative && block.value().name() == "table")
            .then(|| {
                block
                    .select(&count_selector)
                    .next()
                    .map(|p| p.text().collect::<String>().trim().to_owned())
            })
            .flatten()
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
        if let Some(number) = number {
            points += 1;
            ensure!(
                number.parse::<usize>()? == points,
                "Missing, duplicate or out-of-order judgment point"
            );
            nodes.push(node(
                format!("judgment-point-{points}"),
                "judgment_point",
                format!("bod {points}"),
                &heading,
                text,
            ));
        } else if let Some(last) = nodes.last_mut() {
            operative_body |= operative;
            last.own_text.push('\n');
            last.own_text.push_str(&text);
            last.full_text = last.own_text.clone();
            last.context_text = last.own_text.clone();
        }
    }
    ensure!(
        points == metadata.expected_points && operative && operative_body && ended,
        "Incomplete judgment, operative part or signatures"
    );
    let full_text = complete.join("\n");
    nodes.insert(
        0,
        Node {
            anchor: "judgment".into(),
            parent: None,
            kind: "judgment".into(),
            label: "rozsudok".into(),
            heading: format!(
                "{} · {} · {}",
                manifest.title, metadata.ecli, metadata.legal_context
            ),
            citation: "rozsudok".into(),
            section: None,
            own_text: String::new(),
            context_text: full_text.clone(),
            full_text,
            searchable: false,
            reference_links: vec![],
        },
    );
    Ok(nodes)
}

fn normalized(query: &str) -> String {
    query.to_lowercase().replace(['‑', '–', '−', '‐'], "-")
}

fn case_numbers(query: &str) -> Vec<String> {
    let q = normalized(query);
    let european = Regex::new(r"\b([ctf])\s*-\s*(\d{1,4})\s*/\s*(\d{2,4})\b").unwrap();
    let slovak =
        Regex::new(r"\b(\d{1,3})\s*(\p{L}{1,16})\s*/?\s*(\d{1,6})\s*/\s*(\d{2,4})\b").unwrap();
    let historical =
        Regex::new(r"\b((?:m\s*)?(?:obdo\s*v|nd[ti]|tpj|cpj))\s*/?\s*(\d{1,6})\s*/\s*(\d{2,4})\b")
            .unwrap();
    european
        .captures_iter(&q)
        .map(|c| format!("{}-{}/{}", c[1].to_uppercase(), &c[2], &c[3]))
        .chain(
            slovak
                .captures_iter(&q)
                .map(|c| format!("{}{}/{}/{}", &c[1], &c[2], &c[3], &c[4])),
        )
        .chain(historical.captures_iter(&q).map(|c| {
            format!(
                "{}/{}/{}",
                c[1].split_whitespace().collect::<String>(),
                &c[2],
                &c[3]
            )
        }))
        .collect()
}

fn collection_numbers(query: &str) -> Vec<(u32, i32)> {
    Regex::new(r"(?i)\br\s*([0-9]{1,4})\s*/\s*([0-9]{4})\b")
        .unwrap()
        .captures_iter(query)
        .filter_map(|capture| Some((capture[1].parse().ok()?, capture[2].parse().ok()?)))
        .collect()
}
fn identifiers_match(query: &str, number: &str, metadata: &Judgment) -> bool {
    let q = normalized(query);
    let collection = collection_numbers(query);
    if !collection.is_empty() {
        return metadata
            .collection
            .as_ref()
            .is_some_and(|known| collection.contains(&(known.number, known.year)));
    }
    let numbers = case_numbers(query);
    // Explicit identifiers must not fall back to a coincidental name match.
    if !numbers.is_empty() {
        let known: Vec<_> = std::iter::once(number)
            .chain(metadata.aliases.iter().map(String::as_str))
            .flat_map(case_numbers)
            .collect();
        return numbers.iter().any(|n| known.contains(n));
    }
    if q.contains("ecli:") {
        return Regex::new(r"\becli:[a-z0-9]+:[a-z0-9]+:\d{4}:[a-z0-9]+(?:\.[a-z0-9]+)*\b")
            .unwrap()
            .find_iter(&q)
            .any(|v| v.as_str() == metadata.ecli.to_lowercase());
    }
    if q.contains("celex:") {
        return Regex::new(r"\bcelex:\s*([a-z0-9_]+)\b")
            .unwrap()
            .captures_iter(&q)
            .any(|v| v[1] == metadata.celex.to_lowercase());
    }
    std::iter::once(&metadata.celex)
        .chain(metadata.aliases.iter())
        .map(|a| a.trim())
        .filter(|a| !a.is_empty())
        .any(|alias| {
            Regex::new(&format!(r"(?i)\b{}\b", regex::escape(alias)))
                .unwrap()
                .is_match(&q)
        })
}

pub(crate) fn requested_anchors(query: &str) -> Result<Vec<String>> {
    let q = normalized(query);
    let tail = Regex::new(r"\s*(-|až|az|,|a)\s*([0-9]+)").unwrap();
    let mut anchors = Vec::new();
    for (pattern, prefix) in [
        (
            r"\bbod(?:y|och|u|ov|e)?\s+([0-9]+)((?:\s*(?:-|až|az|,|a)\s*[0-9]+)*)",
            "judgment-point-",
        ),
        (
            r"\b(?:stran(?:a|y|e|u|ách|ach)?|strán)\s+([0-9]+)((?:\s*(?:-|až|az|,|a)\s*[0-9]+)*)",
            "judgment-page-",
        ),
    ] {
        let pattern = Regex::new(pattern).unwrap();
        let mut numbers = Vec::new();
        for capture in pattern.captures_iter(&q) {
            let mut previous: usize = capture[1].parse()?;
            numbers.push(previous);
            for item in tail.captures_iter(&capture[2]) {
                let next: usize = item[2].parse()?;
                if matches!(&item[1], "-" | "až" | "az") {
                    ensure!(
                        next >= previous && next - previous < 20,
                        "At most 20 judgment points or pages per query"
                    );
                    if next > previous {
                        numbers.extend(previous + 1..=next);
                    }
                } else {
                    numbers.push(next);
                }
                previous = next;
            }
            ensure!(
                numbers.len() + anchors.len() <= 20,
                "At most 20 judgment points or pages per query"
            );
        }
        numbers.sort_unstable();
        numbers.dedup();
        anchors.extend(numbers.into_iter().map(|n| format!("{prefix}{n}")));
    }
    if anchors.is_empty()
        && Regex::new(r"\b(?:odôvodneni[aeu]|odovodneni[aeu]|odůvodnění)\b")
            .unwrap()
            .is_match(&q)
    {
        anchors.push("judgment-reasoning".into());
    }
    if q.contains("výrok") || q.contains("vyrok") {
        anchors.push("judgment-operative".into());
    }
    if Regex::new(r"\bpr[áa]vn(?:a|u|ej|e|ych|ých)\s+(?:vet(?:a|u|y|e|ou|ách|ach)|viet)\b")
        .unwrap()
        .is_match(&q)
    {
        anchors.push("judgment-headnote".into());
    }
    ensure!(
        anchors.len() <= 20,
        "At most 20 judgment references per query"
    );
    if anchors.is_empty() {
        anchors.push("judgment".into());
    }
    Ok(anchors)
}

fn judgment_anchors(query: &str, metadata: &Judgment) -> Result<Vec<String>> {
    let mut anchors = requested_anchors(query)?;
    // Other importers have no separate reasoning node; keep their previous lookup behavior.
    if !metadata.ecli.starts_with("ECLI:CZ:NS:") {
        anchors.retain(|anchor| anchor != "judgment-reasoning");
        if anchors.is_empty() {
            anchors.push("judgment".into());
        }
    }
    Ok(anchors)
}

pub async fn references(pool: &PgPool, query: &str) -> Result<Option<Vec<(String, String)>>> {
    let rows: Vec<(String, String, sqlx::types::Json<Judgment>)> = sqlx::query_as(
        "SELECT id,number,judgment FROM acts WHERE judgment IS NOT NULL ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    let q = normalized(query);
    let mut explicit = !case_numbers(query).is_empty()
        || !collection_numbers(query).is_empty()
        || q.contains("ecli:")
        || q.contains("celex:");
    let mut results = Vec::new();
    for (id, number, metadata) in rows {
        if identifiers_match(query, &number, &metadata) {
            explicit = true;
            for anchor in judgment_anchors(query, &metadata)? {
                results.push((id.clone(), anchor));
            }
        }
    }
    ensure!(
        results.len() <= 20,
        "At most 20 judgment references per query"
    );
    Ok(explicit.then_some(results))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn czech_case_numbers_ecli_and_reasoning_are_exact() {
        let html = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/nscr-33cdo314-2023.html"
        ));
        let m = super::super::czech_supreme_court::manifest(
            html,
            "",
            chrono::NaiveDate::from_ymd_opt(2026, 10, 13).unwrap(),
        )
        .unwrap();
        let j = m.judgment.as_ref().unwrap();
        for query in ["33 Cdo 314/2023", "33Cdo314/2023 odôvodnenie", &j.ecli] {
            assert!(identifiers_match(query, &m.number, j));
        }
        for query in ["33 Cdo 315/2023", "ECLI:CZ:NS:2023:33.CDO.314.2023.10"] {
            assert!(!identifiers_match(query, &m.number, j));
        }
        assert_eq!(
            requested_anchors("33 Cdo 314/2023 odôvodnenie").unwrap(),
            ["judgment-reasoning"]
        );
        assert_eq!(
            requested_anchors("33 Cdo 314/2023 bod 2 odôvodnenie").unwrap(),
            ["judgment-point-2"]
        );
        let mut domestic = j.clone();
        domestic.ecli = "ECLI:SK:NSSR:2024:0021203612.1".into();
        assert_eq!(
            judgment_anchors("4Ndc/31/2024 odôvodnenie", &domestic).unwrap(),
            ["judgment"]
        );
        assert_eq!(
            judgment_anchors("4Ndc/31/2024 strana 2 odôvodnenie", &domestic).unwrap(),
            ["judgment-page-2"]
        );
    }
    #[test]
    fn collection_identity_headnotes_and_historical_cases_are_exact() {
        let mut m = fixture().judgment.unwrap();
        m.ecli.clear();
        m.celex.clear();
        m.aliases = vec![
            "Ndt122/99".into(),
            "1Cdo1/01".into(),
            "ObdoV57/99".into(),
            "MObdoV2/2000".into(),
            "Náhrada škody".into(),
        ];
        m.collection = Some(serde_json::from_value(serde_json::json!({"issue":"2/2002","year":2002,"number":20,"first_page":9,"last_page":9,"ocr":true})).unwrap());
        for query in [
            "R20/2002",
            "R 20 / 2002 právna veta",
            "Ndt 122 / 99",
            "1 Cdo 1/01",
            "Obdo V 57/99",
            "MObdoV2/2000",
        ] {
            assert!(identifiers_match(query, "5To61/2000", &m), "{query}");
        }
        for query in [
            "R120/2002 Náhrada škody",
            "R20/2003 Náhrada škody",
            "R21/2002 Ndt122/99",
            "Ndt1220/99 Náhrada škody",
            "MObdoV20/2000 Náhrada škody",
        ] {
            assert!(!identifiers_match(query, "5To61/2000", &m), "{query}");
        }
        for query in [
            "R20/2002 právna veta",
            "R20/2002 pravna veta",
            "R20/2002 z právnej vety",
            "R20/2002 právnu vetu",
        ] {
            assert_eq!(
                requested_anchors(query).unwrap(),
                ["judgment-headnote"],
                "{query}"
            );
        }
        assert_eq!(
            requested_anchors("R20/2002 strana 9 právna veta").unwrap(),
            ["judgment-page-9", "judgment-headnote"]
        );
        assert!(requested_anchors("R20/2002 strany 1 až 20 právna veta").is_err());
    }
    use chrono::NaiveDate;
    fn fixture() -> Manifest {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/gruber-20050120.json"
        )))
        .unwrap()
    }
    #[test]
    fn preserves_all_points_continuations_and_operative_part() {
        let manifest = fixture();
        let nodes = crate::adapters::spi::db::import::parse(
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/data/gruber-20050120.html"
            )),
            &manifest,
        )
        .unwrap();
        assert_eq!(nodes.len(), 58);
        assert_eq!(nodes[0].kind, "judgment");
        assert!(!nodes[0].full_text.contains("Abstrakt rozsudku"));
        assert!(
            nodes[9]
                .full_text
                .contains("spotrebiteľ podnikol v tomto štáte")
        );
        assert!(nodes[27].full_text.contains("6."));
        assert!(nodes[39].full_text.contains("zanedbateľnú úlohu"));
        assert!(nodes[54].full_text.contains("oprávnene vzbudiť dojem"));
        assert!(
            nodes[57]
                .full_text
                .contains("pravidiel osobitnej právomoci")
        );
        assert!(!nodes[56].full_text.contains("rozhodol takto"));
        assert!(nodes.iter().all(|n| n.section.is_none()));
        let broken = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/gruber-20050120.html"
        ))
        .replace("C41DispositifIntroduction", "missing");
        assert!(parse(&broken, &manifest).is_err());
        let broken = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/gruber-20050120.html"
        ))
        .replacen("C01PointnumeroteAltN", "missing", 1);
        assert!(parse(&broken, &manifest).is_err());
    }
    #[test]
    fn exact_case_aliases_and_points_are_bounded() {
        let m = fixture().judgment.unwrap();
        for query in [
            "C-464/01",
            "C‑464/01 bod 39",
            "Johann Gruber proti Bay Wa AG",
            "Gruber",
            "ECLI:EU:C:2005:32",
            "62001CJ0464",
        ] {
            assert!(identifiers_match(query, "C-464/01", &m), "{query}");
        }
        assert!(!identifiers_match("C-999/01 Gruber", "C-464/01", &m));
        assert!(!identifiers_match("Gruberová", "C-464/01", &m));
        assert!(!identifiers_match(
            "ECLI:EU:C:2005:320 Gruber",
            "C-464/01",
            &m
        ));
        assert!(!identifiers_match(
            "CELEX:62001CJ04640 Gruber",
            "C-464/01",
            &m
        ));
        assert_eq!(
            requested_anchors("body 39–41, 46 a 54 Gruber").unwrap(),
            [39, 40, 41, 46, 54].map(|n| format!("judgment-point-{n}"))
        );
        assert_eq!(
            requested_anchors("Gruber výrok").unwrap(),
            ["judgment-operative"]
        );
        assert!(requested_anchors("Gruber body 1 až 56").is_err());
    }
    #[test]
    fn slovak_case_numbers_are_exact_and_ignore_spacing() {
        let mut m = fixture().judgment.unwrap();
        m.ecli = "ECLI:SK:NSSR:2024:1234567890.1".into();
        m.celex.clear();
        m.aliases = vec!["4Cdo/102/2022".into(), "Náhrada škody".into()];
        for query in [
            "4Cdo/102/2022",
            "sp. zn. 4 Cdo / 102 / 2022",
            "4 Cdo 102/2022",
            "4cdo/102/2022 výrok",
        ] {
            assert!(identifiers_match(query, "4Cdo/102/2022", &m), "{query}");
        }
        for query in [
            "14Cdo/102/2022 Náhrada škody",
            "4Cdo/1020/2022 Náhrada škody",
            "4Cdo/103/2022 Náhrada škody",
            "4Cdo/102/2023 Náhrada škody",
            "C-464/01 Náhrada škody",
        ] {
            assert!(!identifiers_match(query, "4Cdo/102/2022", &m), "{query}");
        }
        assert_eq!(case_numbers("3 Sžf / 10 / 2015"), ["3sžf/10/2015"]);
        assert_eq!(case_numbers("1 TdoV 2/2020"), ["1tdov/2/2020"]);
        let eu = fixture().judgment.unwrap();
        assert!(!identifiers_match("4Cdo/102/2022 Gruber", "C-464/01", &eu));
        assert!(identifiers_match("C - 464 / 01", "C-464/01", &eu));
    }
    #[test]
    fn dotted_ecli_is_exact_and_empty_identifiers_never_match() {
        let mut m = fixture().judgment.unwrap();
        m.ecli = "ECLI:SK:NSSR:2024:1234567890.1".into();
        m.celex.clear();
        m.aliases = vec![String::new(), "   ".into(), "Náhrada škody".into()];
        for query in [
            "ECLI:SK:NSSR:2024:1234567890.1",
            "ECLI:SK:NSSR:2024:1234567890.1. bod 3",
            "ecli:sk:nssr:2024:1234567890.1 výrok",
        ] {
            assert!(identifiers_match(query, "4Cdo/102/2022", &m), "{query}");
        }
        for query in [
            "spotrebiteľská zmluva",
            "ECLI:SK:NSSR:2024:1234567890 Náhrada škody",
            "ECLI:SK:NSSR:2024:1234567890.10 Náhrada škody",
            "ECLI:SK:NSSR:2024:1234567890.1a Náhrada škody",
            "ECLI:SK:NSSR:2024:1234567890.1.2 Náhrada škody",
            "CELEX:62001CJ0464 Náhrada škody",
        ] {
            assert!(!identifiers_match(query, "4Cdo/102/2022", &m), "{query}");
        }
        assert!(!identifiers_match(
            "ECLI:EU:C:2005:32.1 Gruber",
            "C-464/01",
            &fixture().judgment.unwrap()
        ));
    }
    #[test]
    fn page_requests_are_bounded_and_distinct_from_points() {
        for query in ["strana 2", "strany 2", "na strane 2", "zo strán 2"] {
            assert_eq!(requested_anchors(query).unwrap(), ["judgment-page-2"]);
        }
        assert_eq!(
            requested_anchors("4Ndc/31/2024 strany 2–4, 6 a 8").unwrap(),
            [2, 3, 4, 6, 8].map(|n| format!("judgment-page-{n}"))
        );
        assert_eq!(
            requested_anchors("4Ndc/31/2024 strana 2 výrok").unwrap(),
            ["judgment-page-2", "judgment-operative"]
        );
        assert_eq!(
            requested_anchors("4Ndc/31/2024 bod 2").unwrap(),
            ["judgment-point-2"]
        );
        assert_eq!(requested_anchors("4Ndc/31/2024").unwrap(), ["judgment"]);
        assert_eq!(
            requested_anchors("strana 10000").unwrap(),
            ["judgment-page-10000"]
        );
        for query in [
            "strany 1 až 21",
            "strany 4 až 2",
            "body 1 až 10 a strany 1 až 11",
            "strany 1 až 20 výrok",
        ] {
            assert!(requested_anchors(query).is_err(), "{query}");
        }
    }
    #[sqlx::test(migrations = "./migrations")]
    #[ignore = "requires disposable PostgreSQL via DATABASE_URL"]
    async fn import_is_idempotent_and_points_receive_full_judgment_context(pool: PgPool) {
        let manifest = fixture();
        let html = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/gruber-20050120.html"
        ));
        crate::adapters::spi::db::import::import(&pool, html, &manifest)
            .await
            .unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let before =
            crate::adapters::spi::db::search::search(&pool, None, "Gruber bod 39", date, 6)
                .await
                .unwrap();
        assert_eq!(before.len(), 1);
        crate::adapters::spi::db::import::import(&pool, html, &manifest)
            .await
            .unwrap();
        let after =
            crate::adapters::spi::db::search::search(&pool, None, "C-464/01 bod 39", date, 6)
                .await
                .unwrap();
        assert_eq!(before[0].id, after[0].id);
        assert!(after[0].judgment.is_some());
        let mut context = crate::application::usecases::context::EvidenceContext::default();
        context
            .add(
                &crate::adapters::spi::db::PgLegalRepository::new(pool.clone(), None),
                after,
                date,
            )
            .await
            .unwrap();
        assert_eq!(context.sources.len(), 2);
        let whole = context
            .sources
            .iter()
            .find(|s| s.kind == "judgment")
            .unwrap();
        assert!(whole.text.contains("Pravidlá právomoci uvedené"));
        assert!(whole.text.contains("Podpisy"));
        assert!(!whole.source_url.contains('#'));
        let searchable: i64 = sqlx::query_scalar("SELECT count(*) FROM legal_embedding_inputs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(searchable, 57);
    }
}
