use crate::adapters::spi::db::import::{Manifest, Node};
use anyhow::{Context, Result, ensure};
use regex::Regex;
use scraper::{Html, Selector};

pub const ACT_ID: &str = "EU/TREATY/41998A0126";
const ALIAS: &str =
    r"(?i)\b(?:bd|bruselsk\w*\s+dohovor\w*|brussels\s+convention|(?:celex:\s*)?41998A0126)\b";

pub use crate::domain::Treaty;

pub fn matches(query: &str) -> bool {
    Regex::new(ALIAS).unwrap().is_match(query)
}

// Search vocabulary only: retain the original source text and citations.
// Once the act is scoped, its shared title must not dominate every result.
pub fn concept_query(query: &str) -> String {
    let mut result = Regex::new(ALIAS)
        .unwrap()
        .replace_all(query, " ")
        .to_string();
    for (pattern, english) in [
        (r"(?i)spotrebite[lľ]", "consumer"),
        (r"(?i)zmluv", "contract"),
        (r"(?i)pr[aá]vomoc", "jurisdiction"),
        (r"(?i)bydlisk", "domicile"),
        (r"(?i)poisten", "insurance"),
        (r"(?i)rozhodnut", "judgment"),
        (r"(?i)v[yý]kon|vym[aá]han", "enforcement"),
        (r"(?i)uznan|uzn[aá]van", "recognition"),
        (r"(?i)predbe[zž]n", "provisional"),
    ] {
        if Regex::new(pattern).unwrap().is_match(query) {
            result.push(' ');
            result.push_str(english);
        }
    }
    result.trim().to_string()
}

pub fn whole_document(query: &str) -> bool {
    let q = query.trim().trim_end_matches(['.', '?']);
    Regex::new(ALIAS)
        .unwrap()
        .find(q)
        .is_some_and(|m| m.start() == 0 && m.end() == q.len())
}

pub fn article_anchor(query: &str, number: &str) -> String {
    let q = query.to_lowercase();
    let prefix = if q.contains("protokol") || q.contains("protocol") {
        if q.contains("výklad") || q.contains("vyklad") || q.contains("interpretation") {
            "treaty-interpretation.clanok-"
        } else {
            "treaty-protocol.clanok-"
        }
    } else {
        "predpis.text2.clanok-"
    };
    format!("{prefix}{}", number.to_lowercase())
}

// This official legacy HTML has no article anchors. Our IDs are local only.
// Keep the original paragraphs, including notes/declarations, in the root;
// partition only the actual article blocks into independently searchable nodes.
pub fn parse(html: &str, manifest: &Manifest) -> Result<Vec<Node>> {
    let metadata = manifest
        .treaty
        .as_ref()
        .context("Missing treaty metadata")?;
    ensure!(
        metadata.published_on == manifest.valid_from && manifest.valid_to.is_none(),
        "Invalid treaty publication date"
    );
    ensure!(
        metadata.language == "en" && metadata.celex == "41998A0126",
        "Unsupported treaty snapshot"
    );
    let document = Html::parse_document(html);
    let selector = Selector::parse("p").unwrap();
    let rows: Vec<String> = document
        .select(&Selector::parse("#TexteOnly p").unwrap())
        .filter(|p| p.select(&selector).next().is_none())
        .map(|p| {
            p.text()
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|s| !s.is_empty())
        .collect();
    let complete = rows.join("\n");
    ensure!(
        complete.contains("this text has no binding force"),
        "Missing consolidation status notice"
    );
    let mut nodes = vec![node(
        "treaty",
        None,
        "treaty",
        "dohovor a protokoly",
        &manifest.title,
        complete,
        false,
    )];
    let headings = Regex::new(r"^Article (\d+[a-z]?|[IVX]+[a-e]?)(?: \(\d+\))?$").unwrap();
    for (marker, id, title, expected) in [
        (
            "CONVENTION on jurisdiction",
            "treaty-convention",
            "Bruselský dohovor (1968)",
            metadata.expected_convention_articles,
        ),
        (
            "PROTOCOL (",
            "treaty-protocol",
            "Protokol pripojený k Bruselskému dohovoru (1968)",
            metadata.expected_protocol_articles,
        ),
        (
            "PROTOCOL on the interpretation",
            "treaty-interpretation",
            "Protokol o výklade Bruselského dohovoru (1971)",
            metadata.expected_interpretation_articles,
        ),
    ] {
        let start = rows
            .iter()
            .position(|row| row.starts_with(marker))
            .context("Missing treaty component")?;
        let end = rows
            .iter()
            .enumerate()
            .skip(start)
            .find(|(_, row)| row.starts_with("In witness whereof"))
            .map(|(i, _)| i)
            .context("Missing component signature boundary")?;
        nodes.push(node(
            id,
            Some("treaty"),
            "treaty_component",
            title,
            title,
            rows[start..end].join("\n"),
            false,
        ));
        let before = nodes.len();
        let mut active: Option<usize> = None;
        let mut context = Vec::new();
        let mut structural_heading = false;
        let mut labels = Vec::new();
        for row in &rows[start + 1..end] {
            if let Some(capture) = headings.captures(row) {
                let number = &capture[1];
                labels.push(number.to_string());
                let anchor = if id == "treaty-convention" {
                    format!("predpis.text2.clanok-{}", number.to_lowercase())
                } else {
                    format!("{id}.clanok-{}", number.to_lowercase())
                };
                let citation = if id == "treaty-convention" {
                    format!("Čl. {number}")
                } else {
                    format!("Čl. {number} – {title}")
                };
                nodes.push(node(
                    &anchor,
                    Some(id),
                    "article",
                    &citation,
                    &format!("{title} · {}", context.join(" · ")),
                    row.clone(),
                    true,
                ));
                active = Some(nodes.len() - 1);
                structural_heading = false;
            } else if row.starts_with("TITLE ") || row.starts_with("Section ") {
                active = None;
                if row.starts_with("TITLE ") {
                    context.clear();
                } else {
                    // Only the current section belongs to this article's title path.
                    context.truncate(2);
                }
                context.push(row.clone());
                structural_heading = true;
            } else if let Some(index) = active {
                let article = &mut nodes[index];
                article.own_text.push('\n');
                article.own_text.push_str(row);
                article.full_text = article.own_text.clone();
                article.context_text = article.own_text.clone();
            } else if structural_heading {
                context.push(row.clone());
                structural_heading = false;
            }
        }
        ensure!(
            nodes.len() - before == expected,
            "Incomplete treaty component: {id}"
        );
        let expected_labels: Vec<String> = match id {
            "treaty-convention" => (1..=68)
                .flat_map(|n| {
                    if [6, 12, 54].contains(&n) {
                        vec![n.to_string(), format!("{n}a")]
                    } else {
                        vec![n.to_string()]
                    }
                })
                .collect(),
            "treaty-protocol" => [
                "I", "II", "III", "IV", "V", "Va", "Vb", "Vc", "Vd", "Ve", "VI",
            ]
            .map(str::to_string)
            .to_vec(),
            _ => (1..=14).map(|n| n.to_string()).collect(),
        };
        ensure!(
            labels == expected_labels,
            "Missing, duplicate or out-of-order treaty article: {id}"
        );
    }
    Ok(nodes)
}

fn node(
    anchor: &str,
    parent: Option<&str>,
    kind: &str,
    citation: &str,
    heading: &str,
    text: String,
    searchable: bool,
) -> Node {
    Node {
        anchor: anchor.into(),
        parent: parent.map(str::to_string),
        kind: kind.into(),
        label: citation.into(),
        citation: citation.into(),
        heading: heading.into(),
        section: None,
        own_text: if searchable {
            text.clone()
        } else {
            String::new()
        },
        full_text: text.clone(),
        context_text: text,
        searchable,
        reference_links: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Manifest {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/brussels-19980126.json"
        )))
        .unwrap()
    }

    #[test]
    fn all_articles_are_preserved_without_mixing_protocols_or_notes() {
        let manifest = fixture();
        let html = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/brussels-19980126.html"
        ));
        let nodes = crate::adapters::spi::db::import::parse(html, &manifest).unwrap();
        assert_eq!(nodes.len(), 100);
        let article = |anchor: &str| nodes.iter().find(|n| n.anchor == anchor).unwrap();
        let consumer = article("predpis.text2.clanok-13");
        assert!(
            consumer
                .full_text
                .contains("outside his trade or profession")
        );
        assert!(consumer.full_text.contains("specific invitation"));
        assert!(consumer.heading.contains("Section 4"));
        assert!(!consumer.heading.contains("Section 3"));
        assert!(consumer.full_text.contains("contracts of transport"));
        assert!(
            !consumer
                .full_text
                .contains("A consumer may bring proceedings")
        );
        assert!(
            article("predpis.text2.clanok-14")
                .full_text
                .contains("counter-claim")
        );
        assert!(
            !article("predpis.text2.clanok-15")
                .full_text
                .contains("Section 5")
        );
        assert!(
            article("treaty-interpretation.clanok-13")
                .full_text
                .contains("revision of this Protocol")
        );
        assert!(
            !article("treaty-protocol.clanok-vi")
                .full_text
                .contains("JOINT DECLARATION")
        );
        assert!(
            !article("treaty-interpretation.clanok-14")
                .full_text
                .contains("JOINT DECLARATION")
        );
        assert!(
            nodes[0]
                .full_text
                .contains("JOINT DECLARATION of 9 October 1978")
        );
        assert!(
            nodes[0]
                .full_text
                .contains("this text has no binding force")
        );
        assert_eq!(nodes.iter().filter(|n| n.searchable).count(), 96);
        assert!(parse(&html.replacen("Article 6a", "Article 6b", 1), &manifest).is_err());
        assert!(whole_document("Bruselský dohovor") && whole_document("CELEX:41998A0126"));
        assert!(
            !matches("Brusel Ia 1215/2012") && !whole_document("spotrebiteľ Bruselský dohovor")
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    #[ignore = "requires disposable PostgreSQL via DATABASE_URL"]
    async fn treaty_search_keeps_identity_dates_and_original_urls(pool: sqlx::PgPool) {
        let manifest = fixture();
        let html = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/brussels-19980126.html"
        ));
        crate::adapters::spi::db::import::import(&pool, html, &manifest)
            .await
            .unwrap();
        crate::adapters::spi::db::import::import(&pool, html, &manifest)
            .await
            .unwrap();
        for (query, anchor) in [
            ("čl. 13 Bruselského dohovoru", "predpis.text2.clanok-13"),
            ("Article 6a Brussels Convention", "predpis.text2.clanok-6a"),
            ("CELEX:41998A0126", "treaty"),
            ("čl. Vd protokolu BD", "treaty-protocol.clanok-vd"),
            (
                "čl. 13 protokolu o výklade BD",
                "treaty-interpretation.clanok-13",
            ),
        ] {
            let sources = crate::adapters::spi::db::search::search(
                &pool,
                None,
                query,
                manifest.verified_on,
                6,
            )
            .await
            .unwrap();
            assert_eq!(sources.len(), 1, "{query}");
            assert_eq!(sources[0].anchor, anchor);
            assert!(sources[0].treaty.is_some());
            assert_eq!(sources[0].source_url, manifest.source_url);
            assert!(!sources[0].citation.contains("zákona č."));
        }
        let natural = crate::adapters::spi::db::search::search(
            &pool,
            None,
            "Aká je právomoc pri spotrebiteľských zmluvách podľa Bruselského dohovoru?",
            manifest.verified_on,
            6,
        )
        .await
        .unwrap();
        assert!(
            natural
                .iter()
                .any(|s| s.anchor == "predpis.text2.clanok-13")
        );
        assert!(
            crate::adapters::spi::db::search::search(
                &pool,
                None,
                "čl. 13 BD",
                manifest.valid_from.pred_opt().unwrap(),
                6
            )
            .await
            .unwrap()
            .is_empty()
        );
        assert!(
            crate::adapters::spi::db::search::search(
                &pool,
                None,
                "čl. 13 BD",
                manifest.valid_from,
                6
            )
            .await
            .unwrap()
            .len()
                == 1
        );
        assert!(
            crate::adapters::spi::db::search::search(
                &pool,
                None,
                "čl. 13 BD",
                manifest.verified_on.succ_opt().unwrap(),
                6
            )
            .await
            .unwrap()
            .len()
                == 1
        );
        assert!(
            crate::adapters::spi::db::search::search(
                &pool,
                None,
                "čl. 13 ods. 99 BD",
                manifest.verified_on,
                6
            )
            .await
            .unwrap()
            .is_empty()
        );
        assert!(
            crate::adapters::spi::db::search::search(
                &pool,
                None,
                "čl. 13 nariadenia 1215/2012",
                manifest.verified_on,
                6
            )
            .await
            .unwrap()
            .is_empty()
        );
    }
}
