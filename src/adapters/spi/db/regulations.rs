use crate::adapters::spi::db::import::{Manifest, Node, ReferenceLink};
use anyhow::{Context, Result, ensure};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};

fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("static selector")
}
fn text(element: ElementRef<'_>) -> String {
    element
        .text()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn links(element: ElementRef<'_>) -> Vec<ReferenceLink> {
    element
        .select(&selector("a[href]"))
        .filter_map(|a| {
            let href = a.value().attr("href")?;
            (href.starts_with("http://") || href.starts_with("https://")).then(|| ReferenceLink {
                href: href.to_owned(),
                label: text(a),
            })
        })
        .collect()
}

// Only structured original Official Journal XHTML is supported. Local article
// anchors are never appended to the official EUR-Lex URL by source serialization.
pub fn parse(html: &str, manifest: &Manifest) -> Result<Vec<Node>> {
    let metadata = manifest
        .regulation
        .as_ref()
        .context("Missing regulation metadata")?;
    ensure!(
        metadata.language == "sk"
            && metadata.published_on == manifest.valid_from
            && manifest.valid_to.is_none(),
        "Invalid regulation publication metadata"
    );
    ensure!(
        Regex::new(r"^3[0-9]{4}R[0-9]{4}$")
            .unwrap()
            .is_match(&metadata.celex)
            && manifest.act_id == format!("EU/REG/{}", metadata.celex),
        "Invalid regulation identity"
    );
    ensure!(
        html.to_ascii_lowercase().contains("</html>"),
        "Incomplete regulation HTML"
    );
    let document = Html::parse_document(html);
    let body = document
        .select(&selector("body"))
        .next()
        .context("Missing regulation body")?;
    let main = body
        .select(&selector(".eli-container"))
        .next()
        .context("Unsupported regulation layout")?;
    ensure!(
        main.select(&selector(".eli-main-title")).count() == 1,
        "Missing regulation title"
    );
    let final_part = main
        .select(&selector("#fnp_1"))
        .next()
        .context("Missing regulation final provisions")?;
    let final_text = text(final_part).to_lowercase();
    ensure!(
        final_text.contains("toto nariadenie je záväzné")
            && final_text.contains("uplatniteľné")
            && final_part
                .select(&selector(".oj-signatory, .signatory"))
                .next()
                .is_some(),
        "Incomplete regulation final provisions or signatures"
    );
    let blocks: Vec<_> = main
        .select(&selector(".eli-subdivision[id^='art_']"))
        .collect();
    let headers = selector(".oj-ti-art, .ti-art");
    ensure!(
        !blocks.is_empty()
            && main.select(&headers).count() == blocks.len()
            && blocks.len() == manifest.expected_articles,
        "Incomplete regulation article inventory"
    );
    let ordered: Vec<_> = main
        .select(&selector(".eli-subdivision[id^='art_'], #fnp_1"))
        .collect();
    ensure!(
        ordered
            .last()
            .is_some_and(|e| e.value().id() == Some("fnp_1")),
        "Invalid regulation order"
    );
    let complete = text(body);
    let mut nodes = vec![Node {
        anchor: "eu-regulation".into(),
        parent: None,
        kind: "eu_regulation".into(),
        label: "nariadenie EÚ".into(),
        heading: format!("{} · {}", manifest.title, metadata.legal_context),
        citation: "nariadenie EÚ".into(),
        section: None,
        own_text: String::new(),
        context_text: String::new(),
        full_text: complete,
        searchable: false,
        reference_links: links(body),
    }];
    let mut notes = std::collections::HashMap::new();
    for note in body.select(&selector("p.oj-note, p.note")) {
        for anchor in note.select(&selector("a[id]")) {
            if let Some(id) = anchor.value().id().filter(|id| id.starts_with("ntr")) {
                ensure!(
                    notes.insert(id.to_owned(), note).is_none(),
                    "Duplicate regulation footnote"
                );
            }
        }
    }
    for (index, block) in blocks.into_iter().enumerate() {
        let number = index + 1;
        ensure!(
            block.value().id() == Some(format!("art_{number}").as_str()),
            "Missing, duplicate or out-of-order regulation article"
        );
        let titles: Vec<_> = block.select(&headers).map(text).collect();
        ensure!(
            titles == [format!("Článok {number}")],
            "Regulation article label does not match its identity"
        );
        let paragraphs = block
            .select(&selector("p"))
            .map(text)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        ensure!(
            paragraphs.len() >= 2 && paragraphs.iter().skip(1).any(|p| p.chars().count() > 10),
            "Empty regulation article"
        );
        ensure!(
            block.select(&selector("img")).next().is_none(),
            "Article contains unsupported graphical text; use the official PDF"
        );
        let full_text = paragraphs.join("\n");
        let mut context_text = full_text.clone();
        let mut reference_links = links(block);
        let mut included = std::collections::HashSet::new();
        for link in block.select(&selector("a[href]")) {
            if let Some(id) = link
                .value()
                .attr("href")
                .and_then(|href| href.strip_prefix('#'))
                .filter(|id| id.starts_with("ntr"))
                && included.insert(id)
            {
                let note = notes
                    .get(id)
                    .context("Missing referenced regulation footnote")?;
                context_text.push_str("\n\n");
                context_text.push_str(&text(*note));
                reference_links.extend(links(*note));
            }
        }
        let citation = format!("Čl. {number}");
        nodes.push(Node {
            anchor: format!("predpis.text2.clanok-{number}"),
            parent: Some("eu-regulation".into()),
            kind: "article".into(),
            label: citation.clone(),
            citation,
            heading: manifest.title.clone(),
            section: None,
            own_text: full_text.clone(),
            context_text,
            full_text,
            searchable: true,
            reference_links,
        });
    }
    let last = nodes.last().context("Missing final regulation article")?;
    ensure!(
        last.full_text.contains("nadobúda účinnosť")
            || last.full_text.contains("nadobudne účinnosť"),
        "Missing regulation entry-into-force article"
    );
    Ok(nodes)
}
