use anyhow::{Context, Result, ensure};
use chrono::NaiveDate;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::collections::HashSet;

pub const BUNDLED_HTML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/data/ssp-20260612.html"
));
pub const BUNDLED_MANIFEST: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/data/manifest.json"));
pub const BUNDLED_CORPORA: &[(&str, &str)] = &[
    (BUNDLED_HTML, BUNDLED_MANIFEST),
    (
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/csp-20260715.html"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/csp-20260715.json"
        )),
    ),
    (
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/cmp-20260817.html"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/cmp-20260817.json"
        )),
    ),
    (
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/sp-20180901.html"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/sp-20180901.json"
        )),
    ),
    (
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/zop-20260901.html"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/zop-20260901.json"
        )),
    ),
    (
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/tz-20260818.html"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/tz-20260818.json"
        )),
    ),
    (
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/tp-20260818.html"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/tp-20260818.json"
        )),
    ),
    (
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/oz-20260731.html"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/oz-20260731.json"
        )),
    ),
    (
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/zos-20260927.html"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/zos-20260927.json"
        )),
    ),
    (
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/gruber-20050120.html"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/gruber-20050120.json"
        )),
    ),
    (
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/brussels-19980126.html"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/brussels-19980126.json"
        )),
    ),
];

#[derive(Debug, Deserialize, Serialize)]
pub struct Manifest {
    #[serde(default)]
    pub judgment: Option<crate::adapters::spi::db::judgments::Judgment>,
    #[serde(default)]
    pub treaty: Option<crate::adapters::spi::db::treaties::Treaty>,
    #[serde(default)]
    pub regulation: Option<crate::domain::Regulation>,
    pub act_id: String,
    pub title: String,
    pub number: String,
    pub valid_from: NaiveDate,
    pub valid_to: Option<NaiveDate>,
    pub verified_on: NaiveDate,
    pub source_url: String,
    pub pdf_url: String,
    pub sha256: String,
    pub expected_sections: usize,
    pub expected_letters: usize,
    #[serde(default)]
    pub expected_articles: usize,
    pub expected_paragraphs: Option<usize>,
    pub expected_nodes: Option<usize>,
}

#[derive(Debug)]
pub struct Node {
    pub anchor: String,
    pub parent: Option<String>,
    pub kind: String,
    pub label: String,
    pub heading: String,
    pub citation: String,
    pub section: Option<String>,
    pub own_text: String,
    pub full_text: String,
    pub context_text: String,
    pub searchable: bool,
    pub reference_links: Vec<ReferenceLink>,
}

pub use crate::domain::ReferenceLink;

fn text(element: ElementRef<'_>) -> String {
    // Concatenate inline text before normalizing whitespace: joining text nodes with
    // spaces would corrupt punctuation around hyperlinks and superscripts.
    element
        .text()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn kind(element: ElementRef<'_>) -> Option<&'static str> {
    [
        ("predpis", "act"),
        ("cast", "part"),
        ("hlava", "chapter"),
        ("diel", "division"),
        ("oddiel", "subdivision"),
        ("skupinaParagrafov", "group"),
        ("skupinaElementov", "group"),
        ("paragraf", "section"),
        ("clanok", "article"),
        ("odsek", "paragraph"),
        ("pismeno", "letter"),
        ("bod", "point"),
        ("priloha", "annex"),
        ("oznacenaPolozka", "annex_item"),
        ("poznamka", "note"),
    ]
    .into_iter()
    .find(|(class, _)| {
        element
            .value()
            .has_class(class, scraper::CaseSensitivity::CaseSensitive)
    })
    .map(|(_, kind)| kind)
}

fn direct_field(element: ElementRef<'_>, suffix: &str) -> String {
    element
        .children()
        .filter_map(ElementRef::wrap)
        .filter(|e| e.value().attr("id").is_some_and(|id| id.ends_with(suffix)))
        .map(text)
        .collect::<Vec<_>>()
        .join(" ")
}

fn full_text(element: ElementRef<'_>) -> String {
    // Preserve block boundaries, even when the HTML source contains no whitespace.
    element
        .children()
        .filter_map(ElementRef::wrap)
        .map(|child| {
            if kind(child).is_some()
                || (child
                    .value()
                    .has_class("text2", scraper::CaseSensitivity::CaseSensitive)
                    && child
                        .descendants()
                        .filter_map(ElementRef::wrap)
                        .any(|e| kind(e).is_some()))
            {
                full_text(child)
            } else {
                text(child)
            }
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn walk(element: ElementRef<'_>, parent: Option<&Node>, nodes: &mut Vec<Node>) {
    let Some(node_kind) = kind(element) else {
        // Principles are articles nested inside Slov-Lex's non-structural text2 wrapper.
        for child in element.children().filter_map(ElementRef::wrap) {
            walk(child, parent, nodes);
        }
        return;
    };
    let Some(anchor) = element.value().id() else {
        return;
    };
    let label = direct_field(element, ".oznacenie");
    let heading = direct_field(element, ".nadpis");
    let own_text = element
        .children()
        .filter_map(ElementRef::wrap)
        .filter(|e| e.value().classes().any(|c| c == "text" || c == "text2"))
        .filter(|e| {
            !e.descendants()
                .filter_map(ElementRef::wrap)
                .any(|child| kind(child).is_some())
        })
        .map(text)
        .collect::<Vec<_>>()
        .join("\n");
    // Quoted replacement text in an amending article belongs to another act.
    // Keep the original anchors, but never index it as this act's own provisions.
    let quotation = element.ancestors().filter_map(ElementRef::wrap).find(|e| {
        e.value()
            .has_class("citat", scraper::CaseSensitivity::CaseSensitive)
    });
    let section = if quotation.is_some() {
        None
    } else if node_kind == "section" {
        Some(anchor.trim_start_matches("paragraf-").to_string())
    } else {
        parent.and_then(|p| p.section.clone())
    };
    let parent_citation = parent.map_or("", |p| p.citation.as_str());
    let mut citation = match node_kind {
        "section" | "article" => label.clone(),
        "paragraph" if !label.is_empty() => {
            format!("{parent_citation} ods. {}", label.trim_matches(['(', ')']))
        }
        "letter" => format!("{parent_citation} písm. {label}"),
        "point" => format!("{parent_citation} bod {}", label.trim_end_matches('.')),
        "note" => format!("Poznámka pod čiarou {label}"),
        "paragraph" => parent_citation.to_string(),
        _ if !label.is_empty() => label.clone(),
        _ => heading.clone(),
    };
    if let Some(quotation) = quotation
        && (node_kind == "section" || !parent_citation.contains(" – citované znenie"))
    {
        let owners: Vec<_> = quotation.ancestors().filter_map(ElementRef::wrap).collect();
        let article = owners
            .iter()
            .find(|e| kind(**e) == Some("article"))
            .map(|e| direct_field(*e, ".oznacenie"))
            .unwrap_or_default();
        let point = owners
            .iter()
            .find(|e| kind(**e) == Some("point"))
            .map(|e| {
                format!(
                    " bod {}",
                    direct_field(*e, ".oznacenie").trim_end_matches('.')
                )
            })
            .unwrap_or_default();
        citation = format!("{article}{point} – citované znenie {citation}");
    }
    // A letter/point always carries the entire containing paragraph, including
    // any closing sentence following the list, not just its introductory phrase.
    let context_element = if matches!(node_kind, "letter" | "point") {
        element
            .ancestors()
            .filter_map(ElementRef::wrap)
            .find(|e| kind(*e) == Some("paragraph"))
            .unwrap_or(element)
    } else {
        element
    };
    let context_text = full_text(context_element);
    // Preserve authoritative link identities instead of guessing their law from
    // a section number in flattened text. Letters also inherit their preamble.
    let mut reference_links = Vec::new();
    if matches!(
        node_kind,
        "section" | "article" | "paragraph" | "letter" | "point" | "note" | "annex_item"
    ) {
        for link in context_element.select(&Selector::parse("a[href]").unwrap()) {
            let reference = ReferenceLink {
                href: link.value().attr("href").unwrap().to_owned(),
                label: text(link),
            };
            if !reference_links.contains(&reference) {
                reference_links.push(reference);
            }
        }
    }
    let searchable = quotation.is_none()
        && matches!(node_kind, "paragraph" | "letter" | "point" | "annex_item")
        && !own_text.is_empty()
        && !own_text.starts_with("Prevziať prílohu");
    let node = Node {
        anchor: anchor.to_string(),
        parent: parent.map(|p| p.anchor.clone()),
        kind: node_kind.to_string(),
        label,
        heading,
        citation,
        section,
        own_text,
        full_text: full_text(element),
        context_text,
        searchable,
        reference_links,
    };
    let start = nodes.len();
    // Keep a separate node alive for recursive context, then insert parent before children.
    for child in element.children().filter_map(ElementRef::wrap) {
        walk(child, Some(&node), nodes);
    }
    nodes.insert(start, node);
}

pub fn parse(html: &str, manifest: &Manifest) -> Result<Vec<Node>> {
    ensure!(
        format!("{:x}", Sha256::digest(html.as_bytes())) == manifest.sha256,
        "Source checksum does not match the reviewed manifest"
    );
    ensure!(
        manifest.verified_on >= manifest.valid_from,
        "Invalid verification date"
    );
    let document = Html::parse_document(html);
    let roots = Selector::parse("#predpis, #prilohy > .priloha, #poznamky > .poznamka").unwrap();
    let mut nodes = Vec::new();
    if manifest
        .judgment
        .as_ref()
        .is_some_and(|j| j.collection.is_some())
    {
        nodes = crate::adapters::spi::db::supreme_court_collection::parse(html, manifest)?;
    } else if manifest
        .judgment
        .as_ref()
        .is_some_and(|j| j.ecli.starts_with("ECLI:SK:NSSR:"))
    {
        nodes = crate::adapters::spi::db::supreme_court::parse(html, manifest)?;
    } else if manifest
        .judgment
        .as_ref()
        .is_some_and(|j| j.ecli.starts_with("ECLI:CZ:NS:"))
    {
        nodes = crate::adapters::spi::db::czech_supreme_court::parse(html, manifest)?;
    } else if manifest.judgment.is_some() {
        nodes = crate::adapters::spi::db::judgments::parse(html, manifest)?;
    } else if manifest.regulation.is_some() {
        nodes = crate::adapters::spi::db::regulations::parse(html, manifest)?;
    } else if manifest.treaty.is_some() {
        nodes = crate::adapters::spi::db::treaties::parse(html, manifest)?;
    } else {
        for root in document.select(&roots) {
            walk(root, None, &mut nodes);
        }
    }
    ensure!(
        nodes.iter().filter(|n| n.kind == "section").count() == manifest.expected_sections,
        "Section count differs from manifest; refusing incomplete import"
    );
    ensure!(
        nodes.iter().filter(|n| n.kind == "letter").count() == manifest.expected_letters,
        "Letter count differs from manifest; refusing incomplete import"
    );
    ensure!(
        nodes.iter().filter(|n| n.kind == "article").count() == manifest.expected_articles,
        "Article count differs from manifest; refusing incomplete import"
    );
    ensure!(
        manifest.expected_paragraphs.is_none_or(|expected| {
            nodes.iter().filter(|n| n.kind == "paragraph").count() == expected
        }),
        "Paragraph count differs from manifest; refusing incomplete import"
    );
    ensure!(
        manifest
            .expected_nodes
            .is_none_or(|expected| nodes.len() == expected),
        "Node count differs from manifest; refusing incomplete import"
    );
    let ids: HashSet<_> = nodes.iter().map(|n| &n.anchor).collect();
    ensure!(ids.len() == nodes.len(), "Duplicate source anchors");
    ensure!(
        nodes
            .iter()
            .all(|n| n.parent.as_ref().is_none_or(|p| ids.contains(p))),
        "Orphan node"
    );
    Ok(nodes)
}

pub async fn import(pool: &PgPool, html: &str, manifest: &Manifest) -> Result<usize> {
    let nodes = parse(html, manifest)?;
    let mut tx = pool.begin().await?;
    // Serialize importers across application replicas; never mutate an existing version.
    sqlx::query("SELECT pg_advisory_xact_lock(1622015)")
        .execute(&mut *tx)
        .await?;
    let count = import_nodes(&mut tx, &nodes, manifest).await?;
    tx.commit().await?;
    Ok(count)
}

/// Validate every entry first and commit the entire issue atomically.
pub async fn import_batch(pool: &PgPool, documents: &[(String, Manifest)]) -> Result<usize> {
    ensure!(!documents.is_empty(), "Zbierka neobsahuje žiadne záznamy.");
    let parsed: Vec<_> = documents
        .iter()
        .map(|(text, manifest)| parse(text, manifest))
        .collect::<Result<_>>()?;
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(1622015)")
        .execute(&mut *tx)
        .await?;
    let mut count = 0;
    for ((_, manifest), nodes) in documents.iter().zip(&parsed) {
        count += import_nodes(&mut tx, nodes, manifest).await?;
    }
    tx.commit().await?;
    Ok(count)
}

async fn import_nodes(
    tx: &mut sqlx::PgConnection,
    nodes: &[Node],
    manifest: &Manifest,
) -> Result<usize> {
    let existing =
        sqlx::query("SELECT id, sha256 FROM act_versions WHERE act_id=$1 AND valid_from=$2")
            .bind(&manifest.act_id)
            .bind(manifest.valid_from)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some(row) = existing {
        ensure!(
            row.get::<String, _>("sha256") == manifest.sha256,
            "Existing version has a different source checksum"
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM legal_nodes WHERE version_id=$1")
            .bind(row.get::<i64, _>("id"))
            .fetch_one(&mut *tx)
            .await?;
        ensure!(count == nodes.len() as i64, "Existing import is incomplete");
        // Backfill derived links for existing snapshots, without changing any
        // official text, node IDs, or embedding hashes. Repeated starts are no-ops.
        let links: Vec<_> = nodes
            .iter()
            .map(|node| {
                serde_json::json!({
                    "anchor": node.anchor, "links": node.reference_links
                })
            })
            .collect();
        sqlx::query("UPDATE legal_nodes n SET reference_links=r.links FROM jsonb_to_recordset($2) AS r(anchor text,links jsonb) WHERE n.version_id=$1 AND n.anchor=r.anchor AND n.reference_links IS DISTINCT FROM r.links")
            .bind(row.get::<i64, _>("id"))
            .bind(serde_json::json!(links))
            .execute(&mut *tx).await?;
        return Ok(count as usize);
    }
    sqlx::query(
        "INSERT INTO acts(id,title,number,judgment,treaty,regulation) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(id) DO NOTHING",
    )
    .bind(&manifest.act_id)
    .bind(&manifest.title)
    .bind(&manifest.number)
    .bind(manifest.judgment.as_ref().map(sqlx::types::Json))
    .bind(manifest.treaty.as_ref().map(sqlx::types::Json))
    .bind(manifest.regulation.as_ref().map(sqlx::types::Json))
    .execute(&mut *tx)
    .await?;
    let version: i64 = sqlx::query_scalar("INSERT INTO act_versions(act_id,valid_from,valid_to,verified_on,source_url,pdf_url,sha256) VALUES($1,$2,$3,$4,$5,$6,$7) RETURNING id")
        .bind(&manifest.act_id).bind(manifest.valid_from).bind(manifest.valid_to).bind(manifest.verified_on)
        .bind(&manifest.source_url).bind(&manifest.pdf_url).bind(&manifest.sha256).fetch_one(&mut *tx).await?;
    for (position, node) in nodes.iter().enumerate() {
        let search_text = format!(
            "{} {} {} {}",
            node.citation,
            node.heading,
            node.section.as_deref().unwrap_or(""),
            node.own_text
        );
        sqlx::query("INSERT INTO legal_nodes(version_id,anchor,parent_anchor,kind,label,heading,citation,section,position,own_text,full_text,context_text,search_text,searchable,reference_links) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,lower(unaccent($13)),$14,$15)")
            .bind(version).bind(&node.anchor).bind(&node.parent).bind(&node.kind).bind(&node.label)
            .bind(&node.heading).bind(&node.citation).bind(&node.section).bind(position as i32)
            .bind(&node.own_text).bind(&node.full_text).bind(&node.context_text).bind(search_text)
            .bind(node.searchable).bind(serde_json::json!(node.reference_links)).execute(&mut *tx).await.context("Inserting legal node")?;
    }
    sqlx::query("SELECT reindex_legal_version($1)")
        .bind(version)
        .execute(&mut *tx)
        .await?;
    tracing::info!(count = nodes.len(), "Official corpus imported");
    Ok(nodes.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[sqlx::test(migrations = "./migrations")]
    #[ignore = "requires PostgreSQL"]
    async fn batch_import_rolls_back_every_entry_on_conflict(pool: PgPool) {
        let pages: Vec<_> = include_str!("../../../../tests/fixtures/nssr-4ndc31-2024.txt")
            .trim_end()
            .trim_end_matches('\u{000c}')
            .split('\u{000c}')
            .map(|page| page.trim().to_owned())
            .collect();
        let date = NaiveDate::from_ymd_opt(2026, 10, 10).unwrap();
        let a = super::super::supreme_court::snapshot_manifest(
            pages.clone(),
            "a".repeat(64),
            "https://www.nsud.sk/data/att/a.pdf",
            date,
        )
        .unwrap();
        let b = super::super::supreme_court::snapshot_manifest(
            pages,
            "b".repeat(64),
            "https://www.nsud.sk/data/att/a.pdf",
            date,
        )
        .unwrap();
        let documents = vec![a, b];
        assert!(import_batch(&pool, &documents).await.is_err());
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM acts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "no partial collection survives a failed transaction"
        );
        assert_eq!(import_batch(&pool, &documents[..1]).await.unwrap(), 6);
        assert_eq!(import_batch(&pool, &documents[..1]).await.unwrap(), 6);
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM legal_nodes")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 6, "retry must not duplicate stored nodes");
    }
    #[test]
    fn all_bundled_acts_preserve_every_structural_source_anchor() {
        for (html, metadata) in BUNDLED_CORPORA {
            let manifest: Manifest = serde_json::from_str(metadata).unwrap();
            let nodes = parse(html, &manifest).unwrap();
            if manifest.judgment.is_some() || manifest.treaty.is_some() {
                continue;
            } // Judgment structure has its own completeness test.
            // Independent flat DOM inventory catches any node skipped by recursive traversal.
            let document = Html::parse_document(html);
            let selector = Selector::parse("div.Skupina[id]").unwrap();
            let expected: HashSet<_> = document
                .select(&selector)
                .map(|e| e.value().id().unwrap().to_string())
                .collect();
            let actual: HashSet<_> = nodes.iter().map(|n| n.anchor.clone()).collect();
            assert_eq!(actual, expected, "{}", manifest.number);
        }
    }

    #[test]
    fn civil_principles_are_searchable_and_keep_article_citations() {
        for (html, metadata) in &BUNDLED_CORPORA[1..3] {
            let mut manifest: Manifest = serde_json::from_str(metadata).unwrap();
            let nodes = parse(html, &manifest).unwrap();
            let article = nodes
                .iter()
                .find(|n| n.anchor == "predpis.text2.clanok-2")
                .unwrap();
            assert_eq!(article.citation, "Čl. 2");
            assert_eq!(article.section, None);
            assert!(article.full_text.contains("(1)\n"));
            let paragraph = nodes
                .iter()
                .find(|n| n.anchor == "predpis.text2.clanok-2.odsek-1")
                .unwrap();
            assert_eq!(paragraph.citation, "Čl. 2 ods. 1");
            assert_eq!(paragraph.parent.as_deref(), Some(article.anchor.as_str()));
            assert!(paragraph.searchable);
            assert!(!paragraph.own_text.is_empty());
            let unnumbered = nodes
                .iter()
                .find(|n| n.anchor == "predpis.text2.clanok-1.odsek-1")
                .unwrap();
            assert_eq!(unnumbered.citation, "Čl. 1");
            assert!(unnumbered.searchable);
            manifest.expected_articles -= 1;
            assert!(parse(html, &manifest).is_err());
        }
    }

    fn corpus() -> Vec<Node> {
        parse(
            BUNDLED_HTML,
            &serde_json::from_str(BUNDLED_MANIFEST).unwrap(),
        )
        .unwrap()
    }
    #[test]
    fn administrative_procedure_keeps_deadlines_letters_and_repeal_points() {
        let (html, metadata) = BUNDLED_CORPORA[3];
        let nodes = parse(html, &serde_json::from_str(metadata).unwrap()).unwrap();
        let deadline = nodes
            .iter()
            .find(|n| n.anchor == "paragraf-54.odsek-2")
            .unwrap();
        assert_eq!(deadline.citation, "§ 54 ods. 2");
        assert!(deadline.searchable);
        assert!(deadline.full_text.contains("15 dní"));
        let letter = nodes
            .iter()
            .find(|n| n.anchor == "paragraf-30.odsek-1.pismeno-a")
            .unwrap();
        assert_eq!(letter.citation, "§ 30 ods. 1 písm. a)");
        assert!(
            letter
                .context_text
                .contains("Správny orgán konanie zastaví")
        );
        assert!(letter.context_text.contains("j)"));
        let point = nodes
            .iter()
            .find(|n| n.anchor == "paragraf-83.bod-1")
            .unwrap();
        assert_eq!(point.citation, "§ 83 bod 1");
        assert_eq!(point.parent.as_deref(), Some("paragraf-83"));
        assert!(point.searchable);
        assert!(point.full_text.contains("91/1960 Zb."));
        assert_eq!(nodes.iter().filter(|n| n.kind == "point").count(), 3);
        assert!(nodes.iter().any(|n| n.anchor == "paragraf-60a"));
    }
    #[test]
    fn criminal_and_offence_acts_preserve_context_annexes_and_notes() {
        for (index, anchor, citation, phrase) in [
            (
                4,
                "paragraf-22.odsek-1.pismeno-b.bod-1",
                "§ 22 ods. 1 písm. b) bod 1",
                "dopravnú nehodu",
            ),
            (
                5,
                "paragraf-212.odsek-1.pismeno-a",
                "§ 212 ods. 1 písm. a)",
                "malú škodu",
            ),
            (
                6,
                "paragraf-34.odsek-1",
                "§ 34 ods. 1",
                "právo odoprieť vypovedať",
            ),
        ] {
            let (html, metadata) = BUNDLED_CORPORA[index];
            let nodes = parse(html, &serde_json::from_str(metadata).unwrap()).unwrap();
            let node = nodes.iter().find(|n| n.anchor == anchor).unwrap();
            assert_eq!(node.citation, citation);
            assert!(node.searchable && node.full_text.contains(phrase));
            if node.kind != "paragraph" {
                assert!(node.context_text.len() > node.full_text.len());
            }
            assert!(nodes.iter().any(|n| n.kind == "annex"));
            if index != 6 {
                let note = nodes.iter().find(|n| n.kind == "note").unwrap();
                assert!(note.citation.starts_with("Poznámka pod čiarou"));
                assert!(!note.own_text.is_empty() && !note.searchable);
                let inserted = if index == 4 { "88aa" } else { "305aa" };
                assert!(
                    nodes
                        .iter()
                        .any(|n| n.anchor == format!("paragraf-{inserted}"))
                );
            }
            if index == 5 {
                assert!(
                    node.context_text
                        .contains("potrestá sa odňatím slobody až na dva roky")
                );
                let paragraph = nodes
                    .iter()
                    .find(|n| n.anchor == "paragraf-212.odsek-1")
                    .unwrap();
                assert!(
                    paragraph
                        .full_text
                        .contains("potrestá sa odňatím slobody až na dva roky")
                );
                let table = nodes
                    .iter()
                    .find(|n| {
                        n.kind == "annex_item"
                            && n.label.starts_with(
                                "Hodnoty ustanovujúce nepatrné množstvo omamných látok",
                            )
                    })
                    .unwrap();
                assert!(table.searchable && table.full_text.contains("Účinná látka"));
            }
        }
    }
    #[test]
    fn civil_code_keeps_unnumbered_text_letter_context_and_transitional_sections() {
        let html = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/oz-20260731.html"
        ));
        let manifest = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/oz-20260731.json"
        )))
        .unwrap();
        let nodes = parse(html, &manifest).unwrap();
        for (anchor, citation, phrase) in [
            (
                "paragraf-420.odsek-1",
                "§ 420 ods. 1",
                "porušením právnej povinnosti",
            ),
            ("paragraf-11.odsek-1", "§ 11", "ochranu svojej osobnosti"),
            (
                "paragraf-53.odsek-4.pismeno-a",
                "§ 53 ods. 4 písm. a)",
                "pred uzavretím zmluvy",
            ),
            ("paragraf-879aa.odsek-1", "§ 879aa", "pred 31. júlom 2026"),
        ] {
            let node = nodes.iter().find(|n| n.anchor == anchor).unwrap();
            assert_eq!(node.citation, citation);
            assert!(node.searchable && node.full_text.contains(phrase));
        }
        let letter = nodes
            .iter()
            .find(|n| n.anchor == "paragraf-53.odsek-4.pismeno-a")
            .unwrap();
        assert!(letter.context_text.contains("Za neprijateľné podmienky"));
        assert!(letter.context_text.contains("b)"));
        assert_eq!(nodes.iter().filter(|n| n.kind == "point").count(), 30);
        assert_eq!(nodes.iter().filter(|n| n.kind == "note").count(), 28);
        assert!(
            nodes
                .iter()
                .filter(|n| n.kind == "note")
                .all(|n| !n.searchable)
        );
        assert!(nodes.iter().any(|n| n.kind == "annex_item" && n.searchable));
    }
    #[test]
    fn consumer_act_keeps_annexes_and_separates_quoted_amendments() {
        let html = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/zos-20260927.html"
        ));
        let metadata = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/zos-20260927.json"
        ));
        let nodes = parse(html, &serde_json::from_str(metadata).unwrap()).unwrap();
        let letter = nodes
            .iter()
            .find(|n| n.anchor == "paragraf-20.odsek-1.pismeno-a")
            .unwrap();
        assert_eq!(letter.citation, "§ 20 ods. 1 písm. a)");
        assert!(letter.searchable && letter.full_text.contains("14 dní"));
        assert!(letter.context_text.contains("30 dní"));
        let point = nodes
            .iter()
            .find(|n| n.anchor == "paragraf-20.odsek-1.pismeno-a.bod-1")
            .unwrap();
        assert_eq!(point.citation, "§ 20 ods. 1 písm. a) bod 1");
        assert!(point.searchable && point.context_text.contains("14 dní"));
        let quoted = nodes
            .iter()
            .find(|n| n.anchor == "paragraf-619.odsek-1")
            .unwrap();
        assert_eq!(
            quoted.citation,
            "Čl. II bod 20 – citované znenie § 619 ods. 1"
        );
        assert!(!quoted.searchable && quoted.section.is_none());
        assert!(quoted.full_text.contains("do dvoch rokov"));
        assert_eq!(
            nodes
                .iter()
                .filter(|n| n.kind == "section" && n.section.is_some())
                .count(),
            60
        );
        assert_eq!(
            nodes
                .iter()
                .filter(|n| n.citation.contains(" – citované znenie") && n.searchable)
                .count(),
            0
        );
        assert_eq!(nodes.iter().filter(|n| n.kind == "annex").count(), 5);
        assert!(
            nodes
                .iter()
                .any(|n| n.anchor.contains("priloha_c_1_") && n.searchable)
        );
        assert!(
            nodes
                .iter()
                .filter(|n| n.kind == "note")
                .all(|n| !n.searchable)
        );
    }
    #[test]
    fn complete_official_corpus_has_unique_nodes_and_letter_context() {
        let nodes = corpus();
        let letter = nodes
            .iter()
            .find(|n| n.anchor == "paragraf-3.odsek-1.pismeno-b")
            .unwrap();
        assert_eq!(letter.citation, "§ 3 ods. 1 písm. b)");
        assert!(
            letter
                .context_text
                .contains("Na účely tohto zákona sa rozumie")
        );
        assert!(
            letter
                .own_text
                .contains("rozhodnutím orgánu verejnej správy")
        );
        let point = nodes
            .iter()
            .find(|n| n.anchor == "paragraf-466.odsek-5.pismeno-b.bod-2")
            .unwrap();
        assert_eq!(point.citation, "§ 466 ods. 5 písm. b) bod 2");
        assert!(point.context_text.contains("Veľký senát"));
        assert_eq!(nodes.iter().filter(|n| n.kind == "paragraph").count(), 971);
        assert_eq!(nodes.iter().filter(|n| n.kind == "point").count(), 2);
    }
    #[test]
    fn unnumbered_paragraph_does_not_invent_a_number() {
        let nodes = corpus();
        assert_eq!(
            nodes
                .iter()
                .find(|n| n.anchor == "paragraf-1.odsek-1.pismeno-a")
                .unwrap()
                .citation,
            "§ 1 písm. a)"
        );
        assert!(nodes.iter().any(|n| n.anchor == "paragraf-493c"));
        let deadline = nodes
            .iter()
            .find(|n| n.anchor == "paragraf-181.odsek-1")
            .unwrap();
        assert!(deadline.full_text.contains("dvoch mesiacov"));
    }
    #[test]
    fn corrupted_snapshot_is_rejected() {
        let manifest: Manifest = serde_json::from_str(BUNDLED_MANIFEST).unwrap();
        assert!(
            parse(
                &BUNDLED_HTML.replace("dvoch mesiacov", "troch mesiacov"),
                &manifest
            )
            .is_err()
        );
    }
}
