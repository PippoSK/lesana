use crate::adapters::spi::db::import::{self, Manifest};
use crate::domain::{Judgment, Regulation};
use anyhow::{Context, Result, bail, ensure};
use chrono::{Datelike, NaiveDate, Utc};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::{collections::HashSet, time::Duration};
use url::Url;

const MAX_BYTES: usize = 20 * 1024 * 1024;
const INVALID_LAW: &str =
    "Zadajte číslo predpisu alebo odkaz na časovú verziu na static.slov-lex.sk.";
const INVALID_REGULATION: &str =
    "Zadajte CELEX pôvodného nariadenia EÚ (napr. 32012R1215) alebo slovenský HTML odkaz EUR-Lex.";
const INVALID_JUDGMENT: &str = "Zadajte CELEX rozsudku alebo slovenský HTML odkaz EUR-Lex.";

#[derive(Debug)]
pub struct ImportOutcome {
    pub act_id: String,
    pub title: String,
    pub node_count: usize,
    pub imported_count: usize,
}

/// A source validation message safe to show to the requesting user.
#[derive(Debug)]
pub struct SourceValidationError(String);
impl std::fmt::Display for SourceValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for SourceValidationError {}
fn source_validation(error: anyhow::Error) -> anyhow::Error {
    SourceValidationError(format!("{error:#}")).into()
}
fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static regular expression")
}
fn selector(pattern: &str) -> Selector {
    Selector::parse(pattern).expect("static selector")
}
fn text(element: ElementRef<'_>) -> String {
    element
        .text()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn iso(value: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").context("Zdroj neobsahuje platný dátum účinnosti.")
}
fn celex_valid(value: &str) -> bool {
    re(r"^6[0-9]{4}[CT]J[0-9]{4}$").is_match(value)
}
fn regulation_celex_valid(value: &str) -> bool {
    re(r"^3[0-9]{4}R[0-9]{4}$").is_match(value)
        && value[1..5]
            .parse::<i32>()
            .is_ok_and(|year| (1950..=2100).contains(&year))
        && value[6..].parse::<u32>().is_ok_and(|number| number > 0)
}
fn judgment_url(celex: &str, format: &str) -> String {
    format!("https://eur-lex.europa.eu/legal-content/SK/{format}/?uri=CELEX:{celex}")
}
fn safe_url(value: &str, host: &str) -> Result<Url> {
    let url = Url::parse(value).context("Neplatný odkaz na oficiálny zdroj.")?;
    ensure!(
        url.scheme() == "https"
            && url.host_str() == Some(host)
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none(),
        "Použite HTTPS odkaz na podporovaný oficiálny zdroj bez prihlasovacích údajov a vlastného portu."
    );
    Ok(url)
}

/// Normalize only supported official identities; arbitrary destinations are never fetched.
pub fn validate_reference(kind: &str, reference: &str) -> Result<String> {
    let reference = reference.trim();
    ensure!(
        !reference.is_empty() && reference.len() <= 2048,
        "Neplatná referencia."
    );
    match kind {
        "legislation" => {
            if let Some(c) = re(r"(?i)^([1-9]\d{0,3})\s*/\s*(\d{4})(?:\s+(?:Z\.\s*z\.|Zb\.))?$")
                .captures(reference)
            {
                ensure!(
                    (1800..=2100).contains(&c[2].parse::<i32>()?),
                    "{INVALID_LAW}"
                );
                return Ok(format!(
                    "https://static.slov-lex.sk/static/SK/ZZ/{}/{}/",
                    &c[2], &c[1]
                ));
            }
            let mut url = safe_url(reference, "static.slov-lex.sk")?;
            ensure!(url.query().is_none(), "{INVALID_LAW}");
            let c = re(r"^/static/SK/ZZ/(\d{4})/([1-9]\d{0,3})/(?:([0-9]{8})\.html)?$")
                .captures(url.path())
                .context(INVALID_LAW)?;
            ensure!(
                (1800..=2100).contains(&c[1].parse::<i32>()?),
                "{INVALID_LAW}"
            );
            if let Some(date) = c.get(3) {
                NaiveDate::parse_from_str(date.as_str(), "%Y%m%d").context(INVALID_LAW)?;
            }
            url.set_fragment(None);
            Ok(url.to_string())
        }
        "supreme_court_judgment" | "supreme_court_collection" => {
            let host = Url::parse(reference)?.host_str().unwrap_or("").to_owned();
            ensure!(
                matches!(host.as_str(), "nsud.sk" | "www.nsud.sk"),
                "Použite priamy PDF odkaz na rozhodnutie alebo čiastku zbierky na www.nsud.sk."
            );
            let mut url = safe_url(reference, &host)?;
            ensure!(
                url.query().is_none()
                    && re(r"^/data/att/(?:[A-Za-z0-9_-]+/)*[A-Za-z0-9_.-]+\.pdf$")
                        .is_match(url.path()),
                "Použite priamy PDF odkaz na rozhodnutie alebo čiastku zbierky na www.nsud.sk."
            );
            url.set_host(Some("www.nsud.sk"))?;
            url.set_fragment(None);
            Ok(url.to_string())
        }
        "czech_supreme_court_judgment" => {
            const INVALID: &str = "Zadajte odkaz na konkrétne rozhodnutie NS ČR z rozhodnuti.nsoud.cz (Judikatura/judikatura_ns.nsf/WebSearch/…).";
            let host = Url::parse(reference)?.host_str().unwrap_or("").to_owned();
            ensure!(
                matches!(
                    host.as_str(),
                    "rozhodnuti.nsoud.cz" | "www.nsoud.cz" | "nsoud.cz"
                ),
                "{INVALID}"
            );
            let url = safe_url(reference, &host)?;
            let path =
                re(r"(?i)^/Judikatura/judikatura_ns\.nsf/(?:WebSearch|WebPrint|0)/([a-f0-9]{32})$");
            let identity = path.captures(url.path()).context(INVALID)?;
            // Strip the official UI's highlighting and print variants before deduplication.
            // Never follow arbitrary query parameters or redirects supplied by a user.
            ensure!(
                url.query_pairs().all(|(key, value)| {
                    (key.eq_ignore_ascii_case("opendocument") && value.is_empty())
                        || key.eq_ignore_ascii_case("highlight")
                }),
                "{INVALID}"
            );
            Ok(format!(
                "https://rozhodnuti.nsoud.cz/Judikatura/judikatura_ns.nsf/WebSearch/{}?openDocument",
                identity[1].to_uppercase()
            ))
        }
        "judgment" | "eu_regulation" => {
            let (valid, invalid): (fn(&str) -> bool, &str) = if kind == "eu_regulation" {
                (regulation_celex_valid, INVALID_REGULATION)
            } else {
                (celex_valid, INVALID_JUDGMENT)
            };
            let upper = reference.to_uppercase();
            let value = upper.strip_prefix("CELEX:").unwrap_or(&upper).trim();
            if valid(value) {
                return Ok(judgment_url(value, "TXT/HTML"));
            }
            let url = safe_url(reference, "eur-lex.europa.eu")?;
            ensure!(
                matches!(
                    url.path(),
                    "/legal-content/SK/TXT/HTML/" | "/legal-content/SK/TXT/"
                ),
                "{invalid}"
            );
            let pairs: Vec<_> = url.query_pairs().collect();
            ensure!(pairs.len() == 1 && pairs[0].0 == "uri", "{invalid}");
            let value = pairs[0].1.to_uppercase();
            let celex = value.strip_prefix("CELEX:").context(invalid)?;
            ensure!(valid(celex), "{invalid}");
            Ok(judgment_url(celex, "TXT/HTML"))
        }
        _ => bail!("Vyberte predpis, judikatúru alebo nariadenie EÚ."),
    }
}

async fn download(client: &reqwest::Client, url: &str) -> Result<String> {
    let mut response = client
        .get(url)
        .header(reqwest::header::ACCEPT_LANGUAGE, "sk,en;q=0.8")
        .send()
        .await
        .context("Oficiálny zdroj sa nepodarilo načítať.")?;
    ensure!(
        response.status().is_success(),
        "Oficiálny zdroj nie je dostupný alebo vyžaduje nepodporované presmerovanie."
    );
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    ensure!(
        content_type.starts_with("text/html") || content_type.starts_with("application/xhtml+xml"),
        "Zdroj neposkytol HTML dokument."
    );
    ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= MAX_BYTES as u64),
        "Dokument je príliš veľký."
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= MAX_BYTES,
            "Dokument je príliš veľký."
        );
        bytes.extend_from_slice(&chunk);
    }
    let html = String::from_utf8(bytes).context("Zdroj nepoužíva UTF-8.")?;
    ensure!(
        html.to_ascii_lowercase().contains("</html>"),
        "Oficiálny dokument je neúplný."
    );
    Ok(html)
}

async fn existing(pool: &PgPool, source_url: &str) -> Result<Option<ImportOutcome>> {
    let result: Option<(String, String, i64)> = sqlx::query_as(
        "SELECT a.id,a.title,count(n.id) FROM acts a JOIN act_versions v ON v.act_id=a.id JOIN legal_nodes n ON n.version_id=v.id WHERE v.source_url=$1 GROUP BY a.id,a.title,v.id HAVING count(n.id)>0")
        .bind(source_url).fetch_optional(pool).await?;
    Ok(result.map(|(act_id, title, count)| ImportOutcome {
        act_id,
        title,
        node_count: count as usize,
        imported_count: 1,
    }))
}

fn cellar_redirect(current: &str, location: &str) -> Result<String> {
    let mut url = Url::parse(current)?.join(location)?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.host_str() == Some("publications.europa.eu")
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path().starts_with("/resource/cellar/"),
        "Neplatné presmerovanie publikačného API."
    );
    // Cellar advertises canonical HTTP identifiers. Always transport over TLS.
    url.set_scheme("https")
        .map_err(|_| anyhow::anyhow!("Invalid scheme"))?;
    Ok(url.into())
}

async fn cellar_download(client: &reqwest::Client, celex: &str, metadata: bool) -> Result<String> {
    ensure!(
        celex_valid(celex) || regulation_celex_valid(celex),
        "Neplatný CELEX oficiálneho dokumentu."
    );
    let mut url = format!("https://publications.europa.eu/resource/celex/{celex}");
    for _ in 0..5 {
        let request = client
            .get(&url)
            .header(
                reqwest::header::ACCEPT,
                if metadata {
                    "application/rdf+xml;notice=non-inferred"
                } else {
                    "application/xhtml+xml,text/html;q=0.9"
                },
            )
            .header("Accept-Max-Cs-Size", MAX_BYTES.to_string());
        // A language on the metadata request selects an expression notice,
        // whereas ECLI and decision date belong to the language-neutral work.
        let request = if metadata {
            request
        } else {
            request.header(reqwest::header::ACCEPT_LANGUAGE, "slk")
        };
        let mut response = request.send().await?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .context("Missing redirect")?
                .to_str()?;
            url = cellar_redirect(&url, location)?;
            continue;
        }
        ensure!(
            response.status() == reqwest::StatusCode::OK,
            "Publikačné API neposkytlo dokument."
        );
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        ensure!(
            if metadata {
                content_type.contains("xml")
            } else {
                content_type.starts_with("text/html")
                    || content_type.starts_with("application/xhtml+xml")
            },
            "Nepodporovaný formát publikačného API."
        );
        ensure!(
            response
                .content_length()
                .is_none_or(|n| n <= MAX_BYTES as u64),
            "Dokument je príliš veľký."
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= MAX_BYTES,
                "Dokument je príliš veľký."
            );
            bytes.extend_from_slice(&chunk);
        }
        let body = String::from_utf8(bytes)?;
        ensure!(
            metadata || body.to_ascii_lowercase().contains("</html>"),
            "Neúplný text oficiálneho dokumentu."
        );
        return Ok(body);
    }
    bail!("Príliš veľa presmerovaní publikačného API.")
}

fn judgment_metadata(xml: &str, celex: &str) -> Result<(String, NaiveDate)> {
    const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
    const CDM: &str = "http://publications.europa.eu/ontology/cdm#";
    let document = roxmltree::Document::parse(xml).context("Neplatné metadáta rozsudku.")?;
    let identity = format!("http://publications.europa.eu/resource/celex/{celex}");
    let work = document
        .root_element()
        .children()
        .find(|node| node.attribute((RDF, "about")) == Some(identity.as_str()))
        .context("Metadáta nepatria k požadovanému rozhodnutiu.")?;
    let property = |name: &str| -> Result<String> {
        let values: HashSet<_> = work
            .children()
            .filter(|n| n.has_tag_name((CDM, name)))
            .filter_map(|n| n.text())
            .map(str::trim)
            .collect();
        ensure!(
            values.len() == 1,
            "Nejednoznačné alebo chýbajúce metadáta rozhodnutia."
        );
        Ok((*values.iter().next().unwrap()).to_owned())
    };
    Ok((
        property("case-law_ecli")?,
        iso(&property("work_date_document")?)?,
    ))
}

fn current_version(html: &str, base: &str, today: NaiveDate) -> Result<String> {
    let document = Html::parse_document(html);
    let base = Url::parse(base)?;
    let mut candidates = Vec::new();
    for row in document.select(&selector(".effectivenessHistoryItem")) {
        if row.value().attr("data-vyhlasene") == Some("1") {
            continue;
        }
        let Some(from) = row
            .value()
            .attr("data-ucinnostod")
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let from = iso(from)?;
        let until = row
            .value()
            .attr("data-ucinnostdo")
            .filter(|s| !s.is_empty())
            .map(iso)
            .transpose()?;
        if from <= today && until.is_none_or(|until| today <= until) {
            candidates.push((
                from,
                base.join(&format!("{}.html", from.format("%Y%m%d")))?
                    .to_string(),
            ));
        }
    }
    candidates.sort();
    candidates.dedup();
    ensure!(
        candidates.len() == 1,
        "Zdroj neuvádza jednoznačné aktuálne účinné znenie. Zadajte priamy odkaz na časovú verziu."
    );
    Ok(candidates.remove(0).1)
}

fn legislation_manifest(html: &str, source: &str, today: NaiveDate) -> Result<Manifest> {
    let url = Url::parse(source)?;
    let c = re(r"^/static/SK/ZZ/(\d{4})/(\d{1,4})/(\d{8})\.html$")
        .captures(url.path())
        .context(INVALID_LAW)?;
    let version = NaiveDate::parse_from_str(&c[3], "%Y%m%d")?;
    let document = Html::parse_document(html);
    ensure!(
        document.select(&selector("#predpis")).count() == 1,
        "Chýba jednoznačný text predpisu."
    );
    let header = document
        .select(&selector(".ucinnost_header"))
        .next()
        .context("Chýba účinnosť predpisu.")?;
    let valid_from = iso(header
        .value()
        .attr("data-from")
        .context("Chýba začiatok účinnosti.")?)?;
    let valid_to = header
        .value()
        .attr("data-to")
        .filter(|s| !s.is_empty())
        .map(iso)
        .transpose()?;
    ensure!(
        valid_from == version && valid_from <= today && valid_to.is_none_or(|to| to >= valid_from),
        "Účinnosť nezodpovedá požadovanej verzii alebo je v budúcnosti."
    );
    let number = document
        .select(&selector("#predpis-header-column h1"))
        .next()
        .map(text)
        .context("Chýba číslo predpisu.")?;
    let identity = re(r"^(\d{1,4})\s*/\s*(\d{4})\b")
        .captures(&number)
        .context("Chýba číslo predpisu.")?;
    ensure!(
        identity[1] == c[2] && identity[2] == c[1],
        "Zdroj vrátil iný predpis."
    );
    let title = document
        .select(&selector("#InfoTable tr"))
        .find_map(|row| {
            let cells: Vec<_> = row.select(&selector("td")).collect();
            (cells.len() >= 2 && text(cells[0]) == "Názov:").then(|| text(cells[1]))
        })
        .or_else(|| {
            document
                .select(&selector(r#"[id="predpis.nadpis"]"#))
                .next()
                .map(text)
        })
        .filter(|s| !s.is_empty())
        .context("Chýba názov predpisu.")?;
    let pdf_href = document
        .select(&selector(".zavaznost a[href]"))
        .next()
        .and_then(|e| e.value().attr("href"))
        .context("Chýba odkaz na záväzné PDF.")?;
    let pdf_url = safe_url(url.join(pdf_href)?.as_str(), "static.slov-lex.sk")?.to_string();
    ensure!(
        pdf_url.ends_with(&format!("ZZ_{}_{}_{}.pdf", &c[1], &c[2], &c[3])),
        "PDF nezodpovedá požadovanému zneniu."
    );
    let inventory: Vec<_> = document.select(&selector("div.Skupina[id]")).collect();
    let count = |class: &str| {
        inventory
            .iter()
            .filter(|node| {
                node.value()
                    .has_class(class, scraper::CaseSensitivity::CaseSensitive)
            })
            .count()
    };
    let manifest = Manifest {
        judgment: None,
        treaty: None,
        regulation: None,
        act_id: format!("SK/ZZ/{}/{}", &c[1], &c[2]),
        title,
        number,
        valid_from,
        valid_to,
        verified_on: today,
        source_url: source.to_owned(),
        pdf_url,
        sha256: format!("{:x}", Sha256::digest(html.as_bytes())),
        expected_sections: count("paragraf"),
        expected_letters: count("pismeno"),
        expected_articles: count("clanok"),
        expected_paragraphs: Some(count("odsek")),
        expected_nodes: Some(inventory.len()),
    };
    let nodes = import::parse(html, &manifest)?;
    ensure!(
        nodes.iter().any(|n| n.searchable),
        "Predpis neobsahuje podporovaný text ustanovení."
    );
    let expected: HashSet<_> = inventory.iter().filter_map(|n| n.value().id()).collect();
    let actual: HashSet<_> = nodes.iter().map(|n| n.anchor.as_str()).collect();
    ensure!(
        actual == expected,
        "Štruktúra predpisu nie je úplne podporovaná."
    );
    Ok(manifest)
}

fn slovak_date(value: &str) -> Result<NaiveDate> {
    let c = re(r"(?i)(\d{1,2})\.\s*(januára|februára|marca|apríla|mája|júna|júla|augusta|septembra|októbra|novembra|decembra)\s+(\d{4})").captures(value).context("Chýba dátum rozhodnutia.")?;
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
        .position(|month| *month == c[2].to_lowercase())
        .unwrap() as u32
        + 1;
    NaiveDate::from_ymd_opt(c[3].parse()?, month, c[1].parse()?)
        .context("Neplatný dátum rozhodnutia.")
}

fn judgment_manifest(
    html: &str,
    metadata: &str,
    source: &str,
    today: NaiveDate,
) -> Result<Manifest> {
    let url = Url::parse(source)?;
    let celex = url
        .query_pairs()
        .find(|(key, _)| key == "uri")
        .context(INVALID_JUDGMENT)?
        .1
        .trim_start_matches("CELEX:")
        .to_string();
    ensure!(celex_valid(&celex), "{INVALID_JUDGMENT}");
    let document = Html::parse_document(html);
    let headers: Vec<_> = document
        .select(&selector("body > p.C19Centre, body > p.sum-title-1"))
        .map(text)
        .collect();
    let court = headers
        .first()
        .filter(|s| s.to_lowercase().contains("rozsudok"))
        .context("Tento formát rozsudku zatiaľ nie je podporovaný.")?
        .clone();
    let decided_on = headers
        .iter()
        .find_map(|s| slovak_date(s).ok())
        .context("Chýba dátum rozsudku.")?;
    ensure!(decided_on <= today, "Dátum rozhodnutia je v budúcnosti.");
    let preamble = document
        .select(&selector("body > p"))
        .take_while(|p| p.value().attr("class") != Some("C01PointnumeroteAltN"))
        .map(text)
        .collect::<Vec<_>>()
        .join(" ")
        .replace(['‑', '–', '−', '‐'], "-");
    let court_letter = &celex[5..6];
    let case_number = format!(
        "{}-{}/{}",
        court_letter,
        celex[7..11].parse::<u32>()?,
        &celex[3..5]
    );
    ensure!(
        re(&format!(
            r"(?i)\b(?:Vec|Vo veci)\s+{}\b",
            regex::escape(&case_number)
        ))
        .is_match(&preamble),
        "Zdroj vrátil iné číslo súdnej veci."
    );
    let (ecli, detail_date) = judgment_metadata(metadata, &celex)?;
    ensure!(
        re(r"^ECLI:EU:[CT]:[0-9]{4}:[0-9]+$").is_match(&ecli),
        "Neplatné ECLI."
    );
    ensure!(
        ecli.starts_with(&format!("ECLI:EU:{court_letter}:{}:", decided_on.year())),
        "ECLI nezodpovedá súdu alebo dátumu rozhodnutia."
    );
    ensure!(
        detail_date == decided_on,
        "Dátum v metadátach nezodpovedá textu rozhodnutia."
    );
    let mut names: Vec<_> = document
        .select(&selector("body > p.S73Alineacentregras"))
        .skip(1)
        .map(text)
        .filter(|name| name != "proti")
        .collect();
    if names.is_empty() {
        // Modern Cellar XHTML puts the parties in bold paragraphs before the
        // court composition and the first table, not in legacy S73 paragraphs.
        names = document
            .select(&selector("body"))
            .next()
            .into_iter()
            .flat_map(|body| body.children().filter_map(ElementRef::wrap))
            .take_while(|element| element.value().name() != "table")
            .filter(|element| element.value().name() == "p")
            .filter_map(|element| {
                let name = element.select(&selector("span.bold")).next().map(text)?;
                (element.value().attr("class") == Some("normal") && text(element) == name)
                    .then(|| name.trim_end_matches(',').to_owned())
            })
            .collect();
    }
    let title = if names.is_empty() {
        format!("{court}, {case_number}")
    } else {
        names.join(" proti ")
    };
    let mut aliases = vec![title.clone()];
    let complete_text = text(document.root_element());
    for name in &names {
        aliases.push(name.trim_end_matches(['.', ',']).to_owned());
        // Only take a short name explicitly defined immediately after this
        // party's full name; generic defined terms must not become case aliases.
        if let Some(capture) = re(&format!(
            r"{}\s*\(ďalej len „([^“]{{3,120}})“\)",
            regex::escape(name)
        ))
        .captures(&complete_text)
        {
            aliases.push(capture[1].to_owned());
        }
    }
    aliases.sort();
    aliases.dedup();
    let legal_context = document
        .select(&selector("body > p.C71Indicateur, body > p.index"))
        .next()
        .map(text)
        .unwrap_or_default();
    let legacy_points = document
        .select(&selector("body > p.C01PointnumeroteAltN"))
        .count();
    let points = if legacy_points > 0 {
        legacy_points
    } else {
        document
            .select(&selector("body > table"))
            .filter_map(|table| table.select(&selector("p.count")).next().map(text))
            .filter(|value| !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
            .count()
    };
    ensure!(
        points > 0,
        "Rozsudok neobsahuje podporované číslované body."
    );
    let manifest = Manifest {
        judgment: Some(Judgment {
            supporting_only: false,
            court,
            ecli,
            celex: celex.clone(),
            language: "sk".into(),
            decided_on,
            legal_context,
            aliases,
            expected_points: points,
            expected_pages: None,
            collection: None,
        }),
        treaty: None,
        regulation: None,
        act_id: format!("EU/CJ/{celex}"),
        title,
        number: case_number,
        valid_from: decided_on,
        valid_to: None,
        verified_on: today,
        source_url: source.into(),
        pdf_url: judgment_url(&celex, "TXT/PDF"),
        sha256: format!("{:x}", Sha256::digest(html.as_bytes())),
        expected_sections: 0,
        expected_letters: 0,
        expected_articles: 0,
        expected_paragraphs: Some(0),
        expected_nodes: Some(points + 2),
    };
    import::parse(html, &manifest)?;
    Ok(manifest)
}

fn regulation_document_date(xml: &str, celex: &str) -> Result<NaiveDate> {
    const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
    const CDM: &str = "http://publications.europa.eu/ontology/cdm#";
    const OWL: &str = "http://www.w3.org/2002/07/owl#";
    let document = roxmltree::Document::parse(xml).context("Neplatné metadáta nariadenia.")?;
    let identity = format!("http://publications.europa.eu/resource/celex/{celex}");
    let descriptions: Vec<_> = document
        .root_element()
        .children()
        .filter(|n| n.has_tag_name((RDF, "Description")))
        .collect();
    let mut identities = HashSet::from([identity.as_str()]);
    // Follow explicit identity equivalence, never properties of cited works.
    loop {
        let before = identities.len();
        for description in &descriptions {
            let Some(subject) = description.attribute((RDF, "about")) else {
                continue;
            };
            let peers: Vec<_> = description
                .children()
                .filter(|n| n.has_tag_name((OWL, "sameAs")))
                .filter_map(|n| n.attribute((RDF, "resource")))
                .collect();
            if identities.contains(subject) || peers.iter().any(|peer| identities.contains(peer)) {
                identities.insert(subject);
                identities.extend(peers);
            }
        }
        if identities.len() == before {
            break;
        }
    }
    let properties: Vec<_> = descriptions
        .iter()
        .filter(|n| {
            n.attribute((RDF, "about"))
                .is_some_and(|id| identities.contains(id))
        })
        .flat_map(|n| n.children())
        .collect();
    let property = |name: &str| -> Result<String> {
        let values: HashSet<_> = properties
            .iter()
            .filter(|n| n.has_tag_name((CDM, name)))
            .filter_map(|n| n.text())
            .map(str::trim)
            .collect();
        ensure!(
            values.len() == 1,
            "Chýbajúce alebo nejednoznačné metadáta nariadenia: {name}"
        );
        Ok((*values.iter().next().unwrap()).to_owned())
    };
    ensure!(
        property("resource_legal_id_celex")? == celex,
        "Metadáta nepatria k požadovanému nariadeniu."
    );
    ensure!(
        property("resource_legal_year")?.parse::<u32>()? == celex[1..5].parse::<u32>()?
            && property("resource_legal_number_natural_celex")?.parse::<u32>()?
                == celex[6..].parse::<u32>()?,
        "Číslo nariadenia v metadátach nezodpovedá CELEX."
    );
    ensure!(
        properties.iter().any(|n| n.has_tag_name((RDF, "type"))
            && n.attribute((RDF, "resource"))
                == Some("http://publications.europa.eu/ontology/cdm#regulation")),
        "Oficiálne metadáta neoznačujú dokument ako nariadenie."
    );
    iso(&property("work_date_document")?)
}

fn regulation_manifest(
    html: &str,
    metadata: &str,
    source: &str,
    today: NaiveDate,
) -> Result<Manifest> {
    let url = Url::parse(source)?;
    let celex = url
        .query_pairs()
        .find(|(key, _)| key == "uri")
        .context(INVALID_REGULATION)?
        .1
        .trim_start_matches("CELEX:")
        .to_owned();
    ensure!(regulation_celex_valid(&celex), "{INVALID_REGULATION}");
    let document = Html::parse_document(html);
    let main = document
        .select(&selector("body .eli-container"))
        .next()
        .context("Tento formát pôvodného nariadenia zatiaľ nie je podporovaný.")?;
    let languages: HashSet<_> = document
        .select(&selector(".oj-hd-lg, .hd-lg"))
        .map(text)
        .collect();
    ensure!(
        languages.len() == 1 && languages.contains("SK"),
        "Zdroj neposkytol slovenské znenie nariadenia."
    );
    let publication_dates: HashSet<_> = document
        .select(&selector(".oj-hd-date, .hd-date"))
        .map(text)
        .collect();
    ensure!(
        publication_dates.len() == 1,
        "Chýba jednoznačný dátum uverejnenia v Úradnom vestníku."
    );
    let published_on =
        NaiveDate::parse_from_str(publication_dates.iter().next().unwrap(), "%d.%m.%Y")
            .context("Neplatný dátum uverejnenia nariadenia.")?;
    let titles: Vec<_> = main
        .select(&selector(
            ".eli-main-title .oj-doc-ti, .eli-main-title .doc-ti",
        ))
        .map(text)
        .filter(|s| !s.is_empty())
        .collect();
    let first_title = titles.first().context("Chýba názov nariadenia.")?;
    ensure!(
        first_title.to_lowercase().contains("nariadenie"),
        "Zdroj neobsahuje názov nariadenia."
    );
    let numbers = re(r"\b([0-9]{1,4})\s*/\s*([0-9]{1,4})\b");
    let number_match = numbers
        .captures(first_title)
        .context("Chýba číslo nariadenia v názve.")?;
    let first = number_match[1].parse::<u32>()?;
    let second = number_match[2].parse::<u32>()?;
    let year = celex[1..5].parse::<u32>()?;
    let serial = celex[6..].parse::<u32>()?;
    ensure!(
        (first == year && second == serial) || (first == serial && second == year),
        "Názov zdroja nezodpovedá požadovanému nariadeniu."
    );
    let title = titles.join(" ");
    let adopted_on = regulation_document_date(metadata, &celex)?;
    ensure!(
        slovak_date(&title)? == adopted_on,
        "Dátum v metadátach nezodpovedá názvu nariadenia."
    );
    ensure!(
        adopted_on <= published_on && published_on <= today,
        "Dátum uverejnenia je pred prijatím alebo v budúcnosti."
    );
    let articles = main
        .select(&selector(".eli-subdivision[id^='art_']"))
        .count();
    let mut legal_context = String::from(
        "Pôvodné slovenské znenie z Úradného vestníka EÚ. Dátum uverejnenia neoznačuje účinnosť ani začiatok uplatňovania. Novely ani konsolidované znenie sa automaticky nedopĺňajú.",
    );
    if document.select(&selector("img")).next().is_some() {
        legal_context.push_str(" Grafické prílohy a formuláre nie sú prevedené na text; ich obsah overte v oficiálnom PDF.");
    }
    let manifest = Manifest {
        judgment: None,
        treaty: None,
        regulation: Some(Regulation {
            celex: celex.clone(),
            language: "sk".into(),
            published_on,
            legal_context,
        }),
        act_id: format!("EU/REG/{celex}"),
        title,
        number: format!("{first}/{second}"),
        valid_from: published_on,
        valid_to: None,
        verified_on: today,
        source_url: source.to_owned(),
        pdf_url: judgment_url(&celex, "TXT/PDF"),
        sha256: format!("{:x}", Sha256::digest(html.as_bytes())),
        expected_sections: 0,
        expected_letters: 0,
        expected_articles: articles,
        expected_paragraphs: Some(0),
        expected_nodes: Some(articles + 1),
    };
    import::parse(html, &manifest)?;
    Ok(manifest)
}

pub async fn fetch_import(pool: &PgPool, kind: &str, reference: &str) -> Result<ImportOutcome> {
    let mut source = validate_reference(kind, reference)?;
    // A collection shares one PDF URL across many records. Never short-circuit
    // its completeness checks on the first record already present at that URL.
    if !matches!(kind, "supreme_court_judgment" | "supreme_court_collection") {
        if let Some(found) = existing(pool, &source).await? {
            return Ok(found);
        }
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .user_agent("Lesana/0.1 (official legal document import)")
        .build()?;
    let today = Utc::now().date_naive();
    if kind == "czech_supreme_court_judgment" {
        let html = download(&client, &source)
            .await
            .map_err(source_validation)?;
        let manifest =
            crate::adapters::spi::db::czech_supreme_court::manifest(&html, &source, today)
                .map_err(source_validation)?;
        let node_count = import::import(pool, &html, &manifest).await?;
        return Ok(ImportOutcome {
            act_id: manifest.act_id,
            title: manifest.title,
            node_count,
            imported_count: 1,
        });
    }
    if matches!(kind, "supreme_court_judgment" | "supreme_court_collection") {
        let (pages, pdf_sha256, ocr) = super::supreme_court_pdf::download_pages(&client, &source)
            .await
            .map_err(source_validation)?;
        use crate::adapters::spi::db::supreme_court_collection as collection;
        if kind == "supreme_court_collection" || collection::is_collection(&pages) {
            let documents = collection::snapshot_manifests(pages, pdf_sha256, &source, today, ocr)
                .map_err(source_validation)?;
            let first = documents
                .first()
                .context("Zbierka neobsahuje rozhodnutia.")?;
            let metadata = first
                .1
                .judgment
                .as_ref()
                .and_then(|j| j.collection.as_ref())
                .context("Chýbajú metadáta zbierky.")?;
            let title = format!("Zbierka stanovísk a rozhodnutí súdov SR {}", metadata.issue);
            let act_id = first.1.act_id.clone();
            let node_count = import::import_batch(pool, &documents).await?;
            return Ok(ImportOutcome {
                act_id,
                title,
                node_count,
                imported_count: documents.len(),
            });
        }
        ensure!(
            !ocr,
            "Samostatné skenované rozhodnutia nie sú podporované; OCR je dostupné pre čiastku zbierky."
        );
        let (snapshot, manifest) = crate::adapters::spi::db::supreme_court::snapshot_manifest(
            pages, pdf_sha256, &source, today,
        )
        .map_err(source_validation)?;
        let node_count = import::import(pool, &snapshot, &manifest).await?;
        return Ok(ImportOutcome {
            act_id: manifest.act_id,
            title: manifest.title,
            node_count,
            imported_count: 1,
        });
    }
    if kind == "legislation" && source.ends_with('/') {
        let history = download(&client, &source).await?;
        source = current_version(&history, &source, today)?;
        if let Some(found) = existing(pool, &source).await? {
            return Ok(found);
        }
    }
    let celex = if matches!(kind, "judgment" | "eu_regulation") {
        Url::parse(&source)?
            .query_pairs()
            .find(|(key, _)| key == "uri")
            .map(|(_, value)| value.trim_start_matches("CELEX:").to_owned())
    } else {
        None
    };
    let html = if let Some(celex) = &celex {
        cellar_download(&client, celex, false).await?
    } else {
        download(&client, &source).await?
    };
    let manifest = if kind == "legislation" {
        legislation_manifest(&html, &source, today)?
    } else {
        let metadata =
            cellar_download(&client, celex.as_deref().context(INVALID_JUDGMENT)?, true).await?;
        if kind == "eu_regulation" {
            regulation_manifest(&html, &metadata, &source, today)?
        } else {
            judgment_manifest(&html, &metadata, &source, today)?
        }
    };
    let node_count = import::import(pool, &html, &manifest).await?;
    Ok(ImportOutcome {
        act_id: manifest.act_id,
        title: manifest.title,
        node_count,
        imported_count: 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn czech_supreme_court_urls_are_official_and_canonical() {
        let kind = "czech_supreme_court_judgment";
        let canonical = "https://rozhodnuti.nsoud.cz/Judikatura/judikatura_ns.nsf/WebSearch/A4D79508F15A7C04C1258A37004CF0D1?openDocument";
        for reference in [
            canonical.to_owned(),
            canonical.replace("rozhodnuti.nsoud.cz", "www.nsoud.cz"),
            canonical
                .replace("rozhodnuti.nsoud.cz", "nsoud.cz")
                .to_lowercase(),
            canonical.replace("WebSearch", "WebPrint"),
            canonical.replace("WebSearch", "0"),
            format!("{canonical}&Highlight=0,narovnani#text"),
        ] {
            assert_eq!(validate_reference(kind, &reference).unwrap(), canonical);
        }
        for reference in [
            canonical.replace("https:", "http:"),
            canonical.replace("rozhodnuti.nsoud.cz", "rozhodnuti.nsoud.cz.evil.example"),
            canonical.replace("rozhodnuti.nsoud.cz", "user@rozhodnuti.nsoud.cz"),
            canonical.replace("rozhodnuti.nsoud.cz", "rozhodnuti.nsoud.cz:444"),
            canonical.replace("judikatura_ns.nsf", "judikatura_vks.nsf"),
            format!("{canonical}&redirect=https://example.com"),
            canonical.replace("?openDocument", "?DeleteDocument"),
            "33 Cdo 314/2023".into(),
            "ECLI:CZ:NS:2023:33.CDO.314.2023.1".into(),
            "https://rozhodnuti.nsoud.cz/".into(),
        ] {
            assert!(validate_reference(kind, &reference).is_err(), "{reference}");
        }
    }
    #[test]
    fn supreme_court_urls_require_official_pdf_and_normalize_identity() {
        let expected = "https://www.nsud.sk/data/att/96d/805912.7b72da.pdf";
        assert_eq!(
            validate_reference("supreme_court_collection", &format!("{expected}#page=12")).unwrap(),
            expected
        );
        for invalid in [
            "https://example.com/collection.pdf",
            "https://www.nsud.sk/data/att/a.pdf?url=http://localhost",
            "https://user:pass@www.nsud.sk/data/att/a.pdf",
        ] {
            assert!(validate_reference("supreme_court_collection", invalid).is_err());
        }
        for value in [
            expected.to_string(),
            format!("{expected}#page=2"),
            expected.replace("www.nsud.sk", "nsud.sk"),
        ] {
            assert_eq!(
                validate_reference("supreme_court_judgment", &value).unwrap(),
                expected
            );
        }
        assert_eq!(
            validate_reference(
                "supreme_court_judgment",
                "https://www.nsud.sk/data/att/66965.pdf"
            )
            .unwrap(),
            "https://www.nsud.sk/data/att/66965.pdf"
        );
        for value in [
            "http://www.nsud.sk/data/att/96d/805912.7b72da.pdf",
            "https://www.nsud.sk.evil.example/data/att/96d/805912.7b72da.pdf",
            "https://user@www.nsud.sk/data/att/96d/805912.7b72da.pdf",
            "https://www.nsud.sk:444/data/att/96d/805912.7b72da.pdf",
            "https://www.nsud.sk/data/att/96d/805912.7b72da.pdf?next=http://localhost",
            "https://www.nsud.sk/data/att/96d/805912.7b72da.html",
            "https://www.nsud.sk/rozhodnutia/4ndc312024/",
            "https://www.nsud.sk/data/att/%2e%2e/file.pdf",
            "file:///etc/passwd",
            "ECLI:SK:NSSR:2024:0021203612.1",
            "4Ndc/31/2024",
        ] {
            assert!(
                validate_reference("supreme_court_judgment", value).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn cellar_redirects_never_leave_official_https_transport() {
        let base = "https://publications.europa.eu/resource/celex/62008CJ0243";
        assert_eq!(
            cellar_redirect(
                base,
                "http://publications.europa.eu/resource/cellar/abc/DOC_1"
            )
            .unwrap(),
            "https://publications.europa.eu/resource/cellar/abc/DOC_1"
        );
        for url in [
            "http://127.0.0.1/resource/cellar/x",
            "https://publications.europa.eu.evil.test/resource/cellar/x",
            "https://u@publications.europa.eu/resource/cellar/x",
            "https://publications.europa.eu:444/resource/cellar/x",
            "https://publications.europa.eu/other",
        ] {
            assert!(cellar_redirect(base, url).is_err());
        }
    }
    #[test]
    fn cellar_xhtml_keeps_all_points_nested_lists_and_operative_part() {
        let html = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/pannon-62008CJ0243.xhtml"
        ));
        let metadata = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:cdm="http://publications.europa.eu/ontology/cdm#"><rdf:Description rdf:about="http://publications.europa.eu/resource/celex/62008CJ0243"><cdm:case-law_ecli>ECLI:EU:C:2009:350</cdm:case-law_ecli><cdm:work_date_document>2009-06-04</cdm:work_date_document></rdf:Description></rdf:RDF>"#;
        let source = judgment_url("62008CJ0243", "TXT/HTML");
        let today = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        let manifest = judgment_manifest(html, metadata, &source, today).unwrap();
        let nodes = import::parse(html, &manifest).unwrap();
        assert_eq!(
            manifest.title,
            "Pannon GSM Zrt. proti Erzsébet Sustikné Győrfi"
        );
        let aliases = &manifest.judgment.as_ref().unwrap().aliases;
        assert!(aliases.iter().any(|alias| alias == "Pannon"));
        assert!(!aliases.iter().any(|alias| alias == "smernica"));
        assert_eq!(nodes.len(), 47);
        assert_eq!(manifest.judgment.as_ref().unwrap().expected_points, 45);
        assert!(
            nodes
                .iter()
                .find(|n| n.anchor == "judgment-operative")
                .unwrap()
                .full_text
                .contains("ex offo")
        );
        assert!(
            nodes
                .iter()
                .find(|n| n.anchor == "judgment-point-2")
                .unwrap()
                .full_text
                .contains("Pannon")
        );
        let corrupt = html.replace("id=\"point2\">2</p>", "id=\"point2\">4</p>");
        assert!(judgment_manifest(&corrupt, metadata, &source, today).is_err());
        assert!(
            judgment_manifest(
                html,
                &metadata.replace("62008CJ0243", "62008CJ0244"),
                &source,
                today
            )
            .is_err()
        );
    }
    #[test]
    fn normalizes_supported_references_and_rejects_untrusted_destinations() {
        assert_eq!(
            validate_reference("legislation", "311/2001 Z. z.").unwrap(),
            "https://static.slov-lex.sk/static/SK/ZZ/2001/311/"
        );
        assert_eq!(
            validate_reference("judgment", "celex:62008CJ0243").unwrap(),
            judgment_url("62008CJ0243", "TXT/HTML")
        );
        for bad in [
            "http://static.slov-lex.sk/static/SK/ZZ/2001/311/",
            "https://static.slov-lex.sk.evil.test/static/SK/ZZ/2001/311/",
            "https://user@static.slov-lex.sk/static/SK/ZZ/2001/311/",
            "https://static.slov-lex.sk:444/static/SK/ZZ/2001/311/",
            "https://static.slov-lex.sk/static/SK/ZZ/2001/311/?url=http://localhost",
            "https://static.slov-lex.sk/static/SK/ZZ/2001/311/20261301.html",
        ] {
            assert!(validate_reference("legislation", bad).is_err(), "{bad}");
        }
        assert!(validate_reference("judgment", "62008CC0243").is_err());
        assert!(validate_reference("judgment", "6۲۰۰۸CJ۰۲۴۳").is_err());
    }

    #[sqlx::test(migrations = "./migrations")]
    #[ignore = "requires PostgreSQL"]
    async fn imported_modern_judgment_resolves_party_name_and_exact_point(pool: PgPool) {
        let html = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/pannon-62008CJ0243.xhtml"
        ));
        let metadata = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:cdm="http://publications.europa.eu/ontology/cdm#"><rdf:Description rdf:about="http://publications.europa.eu/resource/celex/62008CJ0243"><cdm:case-law_ecli>ECLI:EU:C:2009:350</cdm:case-law_ecli><cdm:work_date_document>2009-06-04</cdm:work_date_document></rdf:Description></rdf:RDF>"#;
        let date = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        let manifest = judgment_manifest(
            html,
            metadata,
            &judgment_url("62008CJ0243", "TXT/HTML"),
            date,
        )
        .unwrap();
        import::import(&pool, html, &manifest).await.unwrap();
        for query in ["Pannon bod 32", "Erzsébet Sustikné Győrfi bod 32"] {
            let sources = crate::adapters::spi::db::search::search(&pool, None, query, date, 6)
                .await
                .unwrap();
            assert_eq!(sources.len(), 1, "{query}");
            assert_eq!(sources[0].anchor, "judgment-point-32");
            assert!(
                sources[0]
                    .retrieval_methods
                    .contains(&crate::domain::RetrievalMethod::Exact)
            );
        }
    }
    #[test]
    fn prepares_all_bundled_statutes_from_source_metadata() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        for (html, metadata) in import::BUNDLED_CORPORA {
            let expected: Manifest = serde_json::from_str(metadata).unwrap();
            if expected.judgment.is_some() || expected.treaty.is_some() {
                continue;
            }
            let actual = legislation_manifest(html, &expected.source_url, today).unwrap();
            assert_eq!(actual.act_id, expected.act_id);
            assert_eq!(actual.valid_from, expected.valid_from);
            assert_eq!(actual.expected_sections, expected.expected_sections);
            assert_eq!(
                import::parse(html, &actual).unwrap().len(),
                import::parse(html, &expected).unwrap().len()
            );
        }
    }
    #[test]
    fn rejects_wrong_identity_and_extracts_current_version() {
        let (html, metadata) = import::BUNDLED_CORPORA[0];
        let manifest: Manifest = serde_json::from_str(metadata).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        assert_eq!(
            current_version(
                html,
                "https://static.slov-lex.sk/static/SK/ZZ/2015/162/",
                today
            )
            .unwrap(),
            manifest.source_url
        );
        assert!(
            legislation_manifest(html, &manifest.source_url.replace("/162/", "/999/"), today)
                .is_err()
        );
        assert!(
            current_version(
                html,
                "https://static.slov-lex.sk/static/SK/ZZ/2015/162/",
                NaiveDate::from_ymd_opt(1900, 1, 1).unwrap()
            )
            .is_err()
        );
    }
    #[test]
    fn judgment_metadata_identity_and_date_must_agree() {
        let (html, metadata) = import::BUNDLED_CORPORA[9];
        let expected: Manifest = serde_json::from_str(metadata).unwrap();
        let detail = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:cdm="http://publications.europa.eu/ontology/cdm#"><rdf:Description rdf:about="http://publications.europa.eu/resource/celex/62001CJ0464"><cdm:case-law_ecli>ECLI:EU:C:2005:32</cdm:case-law_ecli><cdm:work_date_document>2005-01-20</cdm:work_date_document></rdf:Description></rdf:RDF>"#;
        let actual = judgment_manifest(
            html,
            detail,
            &expected.source_url,
            NaiveDate::from_ymd_opt(2026, 10, 4).unwrap(),
        )
        .unwrap();
        assert_eq!(actual.number, expected.number);
        assert_eq!(actual.expected_nodes, expected.expected_nodes);
        assert!(
            judgment_manifest(
                html,
                &detail.replace("2005-01-20", "2005-01-21"),
                &expected.source_url,
                actual.verified_on
            )
            .is_err()
        );
        assert!(
            judgment_manifest(
                html,
                &detail.replace("ECLI:EU:C:2005:32", ""),
                &expected.source_url,
                actual.verified_on
            )
            .is_err()
        );
    }
    const REGULATION_HTML: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/regulation-32012R1215.xhtml"
    ));
    const REGULATION_RDF: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/regulation-32012R1215.rdf"
    ));
    #[test]
    fn original_regulation_references_are_canonical_and_bounded() {
        let expected = judgment_url("32012R1215", "TXT/HTML");
        for value in [
            "32012R1215",
            "celex:32012r1215",
            "https://eur-lex.europa.eu/legal-content/SK/TXT/?uri=CELEX:32012R1215",
            "https://eur-lex.europa.eu/legal-content/SK/TXT/HTML/?uri=celex:32012r1215#art_1",
        ] {
            assert_eq!(
                validate_reference("eu_regulation", value).unwrap(),
                expected,
                "{value}"
            );
        }
        for value in [
            "02012R1215-20150226",
            "32012R1215(01)",
            "62008CJ0243",
            "32016L0680",
            "32012R0000",
            "3۲۰۱۲R۱۲۱۵",
            "https://evil.test/legal-content/SK/TXT/?uri=CELEX:32012R1215",
            "https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32012R1215",
            "https://eur-lex.europa.eu/legal-content/SK/TXT/?uri=CELEX:32012R1215&redirect=evil",
            "https://user@eur-lex.europa.eu/legal-content/SK/TXT/?uri=CELEX:32012R1215",
            "https://eur-lex.europa.eu:444/legal-content/SK/TXT/?uri=CELEX:32012R1215",
        ] {
            assert!(
                validate_reference("eu_regulation", value).is_err(),
                "{value}"
            );
        }
    }
    #[test]
    fn original_regulation_keeps_articles_recitals_annex_text_and_publication() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        let source = judgment_url("32012R1215", "TXT/HTML");
        let manifest =
            regulation_manifest(REGULATION_HTML, REGULATION_RDF, &source, today).unwrap();
        let nodes = import::parse(REGULATION_HTML, &manifest).unwrap();
        assert_eq!(manifest.act_id, "EU/REG/32012R1215");
        assert_eq!(manifest.number, "1215/2012");
        assert_eq!(
            manifest.valid_from,
            NaiveDate::from_ymd_opt(2012, 12, 20).unwrap()
        );
        assert_eq!(manifest.expected_articles, 81);
        assert_eq!(nodes.len(), 82);
        assert_eq!(nodes[0].kind, "eu_regulation");
        assert!(nodes[0].full_text.contains("keďže:"));
        assert!(nodes[0].full_text.contains("PRÍLOHA III"));
        assert!(nodes[0].full_text.contains("TABUĽKA ZHODY"));
        assert!(!nodes[0].reference_links.is_empty());
        assert!(
            manifest
                .regulation
                .as_ref()
                .unwrap()
                .legal_context
                .contains("Grafické prílohy")
        );
        assert!(
            nodes[1]
                .full_text
                .contains("občianskych a obchodných veciach")
        );
        assert!(nodes[1].full_text.contains("rozhodcovské konanie"));
        assert!(nodes[81].full_text.contains("10. januára 2015"));
        assert!(nodes[16].context_text.contains("L 335, 17.12.2009"));
        assert!(!nodes[16].full_text.contains("L 335, 17.12.2009"));
        assert!(!nodes[16].reference_links.is_empty());
        assert!(nodes.iter().skip(1).all(|node| node.kind == "article"
            && node.searchable
            && node.parent.as_deref() == Some("eu-regulation")
            && node.section.is_none()));
    }
    #[test]
    fn original_regulation_rejects_wrong_identity_date_language_and_incomplete_structure() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        let source = judgment_url("32012R1215", "TXT/HTML");
        for broken in [
            REGULATION_HTML.replacen("1215/2012", "1216/2012", 1),
            REGULATION_HTML.replace(">SK</p>", ">EN</p>"),
            REGULATION_HTML.replacen("20.12.2012", "11.12.2012", 1),
            REGULATION_HTML.replacen("id=\"art_2\"", "id=\"art_3\"", 1),
            REGULATION_HTML.replacen("Článok 2</p>", "Článok 3</p>", 1),
            REGULATION_HTML.replacen("id=\"fnp_1\"", "id=\"missing\"", 1),
            REGULATION_HTML.replace("</html>", ""),
        ] {
            assert!(regulation_manifest(&broken, REGULATION_RDF, &source, today).is_err());
        }
        let start = REGULATION_HTML
            .find("<div class=\"eli-subdivision\" id=\"art_81\">")
            .unwrap();
        let end = start + REGULATION_HTML[start..].find("</div>").unwrap() + "</div>".len();
        let missing_last = format!("{}{}", &REGULATION_HTML[..start], &REGULATION_HTML[end..]);
        assert!(regulation_manifest(&missing_last, REGULATION_RDF, &source, today).is_err());
        for broken in [
            REGULATION_RDF.replace("32012R1215", "32012R1216"),
            REGULATION_RDF.replace("2012-12-12", "2012-12-13"),
        ] {
            assert!(regulation_manifest(REGULATION_HTML, &broken, &source, today).is_err());
        }
        assert!(
            regulation_manifest(
                REGULATION_HTML,
                REGULATION_RDF,
                &source,
                NaiveDate::from_ymd_opt(2012, 12, 19).unwrap()
            )
            .is_err()
        );
    }
    #[sqlx::test(migrations = "./migrations")]
    #[ignore = "requires disposable PostgreSQL via DATABASE_URL"]
    async fn original_regulation_import_is_idempotent_and_keeps_citations(pool: PgPool) {
        let date = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        let source = judgment_url("32012R1215", "TXT/HTML");
        let manifest = regulation_manifest(REGULATION_HTML, REGULATION_RDF, &source, date).unwrap();
        assert_eq!(
            import::import(&pool, REGULATION_HTML, &manifest)
                .await
                .unwrap(),
            82
        );
        let before = crate::adapters::spi::db::search::search(
            &pool,
            None,
            "čl. 1 CELEX:32012R1215",
            date,
            6,
        )
        .await
        .unwrap();
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].source_url, source);
        assert_eq!(
            before[0].regulation.as_ref().unwrap().published_on,
            manifest.valid_from
        );
        assert_eq!(
            import::import(&pool, REGULATION_HTML, &manifest)
                .await
                .unwrap(),
            82
        );
        let after = crate::adapters::spi::db::search::search(
            &pool,
            None,
            "čl. 1 nariadenia 1215/2012",
            date,
            6,
        )
        .await
        .unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(before[0].id, after[0].id);
        let whole =
            crate::adapters::spi::db::search::search(&pool, None, "CELEX:32012R1215", date, 6)
                .await
                .unwrap();
        assert_eq!(whole.len(), 1);
        assert!(whole[0].text.contains("PRÍLOHA III"));
        let repository = crate::adapters::spi::db::PgLegalRepository::new(pool.clone(), None);
        let mut context = crate::application::usecases::context::EvidenceContext::default();
        assert!(
            !context
                .add(&repository, whole, date)
                .await
                .unwrap()
                .is_empty()
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM legal_embedding_inputs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 81);
    }
}
