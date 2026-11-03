use super::models::SourceRow;
use crate::adapters::spi::db::embedding_index::Embeddings;
use anyhow::{Result, ensure};
use chrono::NaiveDate;
use regex::Regex;
use sqlx::PgPool;

pub use crate::domain::{RetrievalMethod, Source, SourceKind};
const FIELDS: &str = "n.id,n.anchor,n.kind,CASE WHEN a.regulation IS NOT NULL THEN CASE WHEN n.kind='eu_regulation' THEN a.title ELSE n.citation || ' nariadenia EÚ č. ' || a.number END WHEN a.treaty IS NOT NULL THEN n.citation || ', ' || a.number || ' · konsolidácia 1998 (EN)' WHEN a.judgment IS NULL THEN n.citation || ' zákona č. ' || a.number ELSE a.number || ', ' || a.title || ', ' || n.citation END AS citation,n.heading,n.full_text AS text,n.context_text AS context,CASE WHEN n.kind='judgment_page' THEN v.source_url || '#page=' || substring(n.anchor from '[0-9]+$') WHEN a.judgment->'collection'->>'first_page' IS NOT NULL THEN v.source_url || '#page=' || (a.judgment->'collection'->>'first_page') WHEN a.judgment IS NULL AND a.treaty IS NULL AND a.regulation IS NULL THEN v.source_url || '#' || n.anchor ELSE v.source_url END AS source_url,v.pdf_url,a.title AS act_title,a.judgment,a.treaty,a.regulation,v.valid_from,v.valid_to,v.verified_on";
const JOINS: &str =
    "FROM legal_nodes n JOIN act_versions v ON v.id=n.version_id JOIN acts a ON a.id=v.act_id";
// Verification records when the snapshot was checked, not when its text expires.
// Select by known validity and supersession; preserve verified_on in every source.
const DATE_FILTER: &str = "v.valid_from <= $1 AND (v.valid_to IS NULL OR $1 <= v.valid_to) AND NOT EXISTS (SELECT 1 FROM act_versions newer WHERE newer.act_id=v.act_id AND newer.valid_from>v.valid_from AND newer.valid_from <= $1)";

trait SourceKindSql {
    fn filter(self) -> &'static str;
    fn accepts(self, source: &Source) -> bool;
}
impl SourceKindSql for SourceKind {
    // Fixed SQL fragments only; never interpolate model/user arguments into SQL.
    fn filter(self) -> &'static str {
        match self {
            Self::All => "TRUE",
            Self::Legislation => "a.judgment IS NULL",
            Self::Judgments => "a.judgment IS NOT NULL",
        }
    }
    fn accepts(self, source: &Source) -> bool {
        self == Self::All || (source.judgment.is_some() == (self == Self::Judgments))
    }
}

pub fn query_terms(query: &str) -> String {
    let stop = [
        "ako", "ake", "aké", "aký", "aku", "akú", "pre", "pri", "som", "nie", "mám", "mam",
        "mozem", "môžem", "ked", "keď", "ktoré", "ktorý", "tento", "moje", "moju", "prosim",
        "prosím", "chcem", "jeho", "tej", "toho", "bol", "bola", "podľa", "podla",
    ];
    let mut words: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|s| s.chars().count() >= 3 && !stop.contains(&s.as_str()))
        // Prefix matching gives a basic fallback for Slovak inflections without
        // claiming a Slovak stemmer. The model may issue several focused searches.
        .map(|s| {
            format!(
                "{}:*",
                s.chars()
                    .take(if s.chars().count() > 6 {
                        5
                    } else {
                        s.chars().count()
                    })
                    .collect::<String>()
            )
        })
        .take(18)
        .collect();
    words.sort();
    words.dedup();
    words.join(" | ")
}

pub async fn available(pool: &PgPool, date: NaiveDate) -> Result<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM act_versions WHERE valid_from <= $1 AND (valid_to IS NULL OR $1 <= valid_to))")
        .bind(date).fetch_one(pool).await?)
}

// Keep source identity independent of imports: an unknown explicit regulation
// must return no results, never a similarly numbered Slovak act.
fn regulation_scopes(query: &str) -> Vec<(std::ops::Range<usize>, String)> {
    let mut found = Vec::new();
    let celex = Regex::new(r"(?i)\b(?:(?:celex\s*:\s*)|EU/REG/)?([03][0-9]{4}R[0-9A-Z_]+(?:\([0-9A-Z_]+\))?(?:-[0-9A-Z_]+)?)").unwrap();
    for capture in celex.captures_iter(query) {
        found.push((
            capture.get(0).unwrap().range(),
            format!("EU/REG/{}", capture[1].to_uppercase()),
        ));
    }
    let number = Regex::new(r"(?i)\b(?:nariaden\w*|regulation)\s+(?:(?:eur[oó]psk\w*|parlamentu|a|rady|komisie|council|commission|of|the)\s+)*(?:\(?\s*(?:e[uú]|es|ehs|ec|eec)\s*\)?\s*)?(?:č(?:íslo)?\.?\s*|c\.\s*|no\.?\s*)?([0-9]{1,4})\s*/\s*([0-9]{1,4})\b").unwrap();
    for capture in number.captures_iter(query) {
        let (Ok(first), Ok(second)) = (capture[1].parse::<u32>(), capture[2].parse::<u32>()) else {
            continue;
        };
        // Year-like values on both sides can identify two different acts.
        if (2015..=2100).contains(&first) && (1957..2015).contains(&second) {
            found.push((
                capture.get(0).unwrap().range(),
                format!("EU/REG/AMBIGUOUS/{first}/{second}"),
            ));
            continue;
        }
        // Before 2015, the official order was number/year; newer acts use year/number.
        let (year, number) = if (1957..2015).contains(&second) {
            (second, first)
        } else if (2015..=2100).contains(&first) {
            (first, second)
        } else if (1957..=2100).contains(&second) {
            (second, first)
        } else {
            continue;
        };
        if number == 0 {
            continue;
        }
        found.push((
            capture.get(0).unwrap().range(),
            format!("EU/REG/3{year}R{number:04}"),
        ));
    }
    found.sort_by_key(|(range, _)| range.start);
    found
}

// Capture consolidated, corrigendum and dated CELEX tokens in full. A search
// for one must never silently substitute the original regulation or another act.
fn unsupported_regulation_celex(query: &str) -> bool {
    let original = Regex::new(r"^EU/REG/3[0-9]{4}R[0-9]{4}$").unwrap();
    regulation_scopes(query)
        .iter()
        .any(|(_, act)| !original.is_match(act))
}

fn whole_regulations(query: &str) -> Vec<String> {
    let scopes = regulation_scopes(query);
    let mut remainder = query.to_owned();
    for (range, _) in scopes.iter().rev() {
        remainder.replace_range(range.clone(), "");
    }
    if remainder.chars().any(char::is_alphanumeric) {
        return Vec::new();
    }
    let mut acts: Vec<_> = scopes.into_iter().map(|(_, act)| act).collect();
    acts.sort();
    acts.dedup();
    acts
}

fn act_scopes(query: &str) -> Vec<String> {
    let number = Regex::new(r"\b(\d{1,4})\s*/\s*(\d{4})\b").unwrap();
    let regulations = regulation_scopes(query);
    let mut acts: std::collections::HashSet<String> = number
        .captures_iter(query)
        .filter(|capture| {
            !regulations
                .iter()
                .any(|(range, _)| range.contains(&capture.get(0).unwrap().start()))
        })
        .map(|c| format!("SK/ZZ/{}/{}", &c[2], &c[1]))
        .chain(regulations.iter().map(|(_, act)| act.clone()))
        .collect();
    for (pattern, id) in [
        (
            r"(?i)\bcsp\b|\bciviln\w*\s+sporov\w*\s+poriad\w*",
            "SK/ZZ/2015/160",
        ),
        (
            r"(?i)\bcmp\b|\bciviln\w*\s+mimosporov\w*\s+poriad\w*",
            "SK/ZZ/2015/161",
        ),
        (
            r"(?i)\bssp\b|\bspr[aá]vn\w*\s+s[uú]dn\w*\s+poriad\w*",
            "SK/ZZ/2015/162",
        ),
        (
            r"(?i)\bsp\b|\bspr[aá]vn\w*\s+poriad\w*|\bz[aá]kon\w*\s+o\s+spr[aá]vnom\s+konan[ií]\b",
            "SK/ZZ/1967/71",
        ),
        (
            r"(?i)\bzop\b|\bz[aá]kon\w*(?:\s+slovenskej\s+n[aá]rodnej\s+rady)?\s+o\s+priestupkoch\b|\bpriestupkov\w*\s+z[aá]kon\w*",
            "SK/ZZ/1990/372",
        ),
        (r"(?i)\btz\b|\btrestn\w*\s+z[aá]kon\w*", "SK/ZZ/2005/300"),
        (r"(?i)\btp\b|\btrestn\w*\s+poriad\w*", "SK/ZZ/2005/301"),
        (
            r"(?i)\boz\b|\bob[cč]iansk\w*\s+z[aá]konn[ií]k\w*",
            "SK/ZZ/1964/40",
        ),
    ] {
        if Regex::new(pattern).unwrap().is_match(query) {
            acts.insert(id.to_string());
        }
    }
    // The generic title also names the repealed predecessor. Respect an
    // explicit 250/2007 reference instead of silently substituting 108/2024.
    let consumer_alias =
        Regex::new(r"(?i)\bzos\b|\bz[aá]kon\w*\s+o\s+ochrane\s+spotrebite[lľ]a\b").unwrap();
    if consumer_alias.is_match(query) && !acts.contains("SK/ZZ/2007/250") {
        acts.insert("SK/ZZ/2024/108".to_string());
    }
    if crate::adapters::spi::db::treaties::matches(query) {
        acts.insert(crate::adapters::spi::db::treaties::ACT_ID.into());
    }
    let mut acts: Vec<_> = acts.into_iter().collect();
    acts.sort();
    acts
}

fn act_scope(query: &str) -> Option<String> {
    let acts = act_scopes(query);
    if acts.len() == 1 {
        acts.into_iter().next()
    } else {
        None
    }
}

pub fn valid_act(name: &str) -> bool {
    act_scope(name).is_some()
}

pub fn validate_query(query: &str) -> Result<()> {
    references(query)?;
    crate::adapters::spi::db::judgments::requested_anchors(query).map(|_| ())
}

pub(crate) async fn get_provision(
    pool: &PgPool,
    anchor: &str,
    date: NaiveDate,
    act: Option<&str>,
) -> Result<Vec<Source>> {
    let sql = format!(
        "SELECT {FIELDS} {JOINS} WHERE {DATE_FILTER} AND n.anchor=$2 AND (n.anchor NOT LIKE 'paragraf-%' OR n.section IS NOT NULL) AND ($3::text IS NULL OR a.id=$3) ORDER BY a.id"
    );
    let mut sources: Vec<Source> = sqlx::query_as::<_, SourceRow>(&sql)
        .bind(date)
        .bind(anchor)
        .bind(act)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|row| row.0)
        .collect();
    for source in &mut sources {
        source.extend_retrieval_methods(&[RetrievalMethod::Exact]);
    }
    Ok(sources)
}

pub async fn get_section(
    pool: &PgPool,
    section: &str,
    date: NaiveDate,
    act: Option<&str>,
) -> Result<Vec<Source>> {
    ensure!(
        Regex::new(r"^\d{1,4}[a-z]{0,2}$")
            .unwrap()
            .is_match(section),
        "Invalid section number"
    );
    let scope = act
        .map(|name| act_scope(name).ok_or_else(|| anyhow::anyhow!("Unknown or ambiguous act")))
        .transpose()?;
    get_provision(pool, &format!("paragraf-{section}"), date, scope.as_deref()).await
}

#[derive(Debug, PartialEq, Eq)]
struct Reference {
    anchor: String,
    act: Option<String>,
}

// Each explicit reference owns its paragraph/letter suffix. A sole act in the
// whole query can scope a shared list; mixed acts are resolved locally.
fn references(query: &str) -> Result<Vec<Reference>> {
    let pattern = Regex::new(r"(?i)(§{1,2}\s*|paragraf(?:u|e|om)?\s+|\bčl\.?\s*|\bcl\.?\s*|\bčlán(?:ok|ku)\s+|\barticle\s+)(\d{1,4}[a-z]{0,2}|[ivxlcdm]+[a-e]?)\b").unwrap();
    let captures: Vec<_> = pattern.captures_iter(query).collect();
    ensure!(
        captures.len() <= 20,
        "At most 20 explicit references per query"
    );
    let shared_acts = act_scopes(query);
    let first_prefix = captures
        .first()
        .map_or("", |c| &query[..c.get(0).unwrap().start()]);
    let prefix_style = !act_scopes(first_prefix).is_empty() && shared_acts.len() > 1;
    let mut inherited_acts = act_scopes(first_prefix);
    let list = Regex::new(r"(?i)^\s*(?:,\s*(?:a\s+)?|a\s+)(\d{1,4}[a-z]{0,2})\b").unwrap();
    let paragraph = Regex::new(r"(?i)ods(?:ek(?:u|om|e)?)?\.?\s*(\d+)").unwrap();
    let letter = Regex::new(r"(?i)p[ií]sm(?:eno|ena)?\.?\s*([a-z])").unwrap();
    let range = Regex::new(r"^\s*(?:až|az|–|—|-)\s*\d").unwrap();
    let mut result = Vec::new();
    for (index, capture) in captures.iter().enumerate() {
        let end = captures
            .get(index + 1)
            .map_or(query.len(), |next| next.get(0).unwrap().start());
        let mut suffix = &query[capture.get(0).unwrap().end()..end];
        let mut numbers = vec![capture[2].to_lowercase()];
        while let Some(item) = list.captures(suffix) {
            numbers.push(item[1].to_lowercase());
            suffix = &suffix[item.get(0).unwrap().end()..];
        }
        ensure!(
            !range.is_match(suffix),
            "Write each section in a range explicitly"
        );
        let mut scopes = if prefix_style {
            let start = index
                .checked_sub(1)
                .map_or(0, |previous| captures[previous].get(0).unwrap().end());
            let before = act_scopes(&query[start..capture.get(0).unwrap().start()]);
            if !before.is_empty() {
                inherited_acts = before;
            }
            if index + 1 == captures.len() {
                let trailing = act_scopes(suffix);
                ensure!(
                    trailing.is_empty() || trailing == inherited_acts,
                    "Ambiguous act placement; put the act after each reference"
                );
            }
            inherited_acts.clone()
        } else {
            act_scopes(suffix)
        };
        if scopes.is_empty() {
            scopes = shared_acts.clone();
        }
        let scopes: Vec<Option<String>> = if scopes.is_empty() {
            vec![None]
        } else {
            scopes.into_iter().map(Some).collect()
        };
        let prefix =
            if capture[1].starts_with('§') || capture[1].to_lowercase().starts_with("paragraf") {
                "paragraf-"
            } else {
                "predpis.text2.clanok-"
            };
        let p = paragraph.captures(suffix);
        let l = letter.captures(suffix);
        if let Some(p) = &p {
            let tail = &suffix[p.get(0).unwrap().end()..];
            ensure!(
                !list.is_match(tail) && !range.is_match(tail),
                "Repeat the section for each paragraph"
            );
        }
        for number in numbers {
            let mut anchor = format!("{prefix}{number}");
            if let Some(p) = &p {
                anchor.push_str(&format!(".odsek-{}", &p[1]));
            } else if l.is_some() {
                anchor.push_str(".odsek-1");
            }
            if let Some(l) = &l {
                anchor.push_str(&format!(".pismeno-{}", l[1].to_lowercase()));
            }
            for act in &scopes {
                let item = Reference {
                    anchor: if act.as_deref() == Some(crate::adapters::spi::db::treaties::ACT_ID)
                        && prefix == "predpis.text2.clanok-"
                    {
                        format!(
                            "{}{}",
                            crate::adapters::spi::db::treaties::article_anchor(
                                if query[capture.get(0).unwrap().start()..end]
                                    .to_lowercase()
                                    .contains("protokol")
                                    || query[capture.get(0).unwrap().start()..end]
                                        .to_lowercase()
                                        .contains("protocol")
                                {
                                    &query[capture.get(0).unwrap().start()..end]
                                } else {
                                    first_prefix
                                },
                                &number
                            ),
                            &anchor[format!("{prefix}{number}").len()..]
                        )
                    } else {
                        anchor.clone()
                    },
                    act: act.clone(),
                };
                if !result.contains(&item) {
                    result.push(item);
                }
                ensure!(
                    result.len() <= 20,
                    "At most 20 explicit references per query"
                );
            }
        }
    }
    Ok(result)
}

pub async fn get_node(pool: &PgPool, id: i64) -> Result<Option<Source>> {
    let sql = format!("SELECT {FIELDS} {JOINS} WHERE n.id=$1");
    Ok(sqlx::query_as::<_, SourceRow>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .map(|row| row.0))
}

/// Nearest complete section/article in the same version as the retrieved node.
pub async fn containing_provision(
    pool: &PgPool,
    id: i64,
    date: NaiveDate,
) -> Result<Option<Source>> {
    let sql = format!(
        "WITH RECURSIVE ancestors AS (
            SELECT id,version_id,anchor,parent_anchor,kind,0 AS depth FROM legal_nodes WHERE id=$2
            UNION ALL
            SELECT p.id,p.version_id,p.anchor,p.parent_anchor,p.kind,c.depth+1
            FROM ancestors c JOIN legal_nodes p ON p.version_id=c.version_id AND p.anchor=c.parent_anchor
        ) SELECT {FIELDS} {JOINS} JOIN ancestors r ON r.id=n.id
          WHERE {DATE_FILTER} AND n.kind IN ('section','article','judgment') ORDER BY r.depth LIMIT 1"
    );
    Ok(sqlx::query_as::<_, SourceRow>(&sql)
        .bind(date)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .map(|row| row.0))
}

pub async fn reference_links(
    pool: &PgPool,
    id: i64,
) -> Result<Vec<crate::adapters::spi::db::import::ReferenceLink>> {
    let links: sqlx::types::Json<Vec<crate::adapters::spi::db::import::ReferenceLink>> =
        sqlx::query_scalar("SELECT reference_links FROM legal_nodes WHERE id=$1")
            .bind(id)
            .fetch_one(pool)
            .await?;
    Ok(links.0)
}

/// Include inserted sections (e.g. § 5a) in an official section range.
pub async fn section_range(
    pool: &PgPool,
    start: &str,
    end: &str,
    act: &str,
    date: NaiveDate,
) -> Result<Vec<Source>> {
    let sql = format!(
        "SELECT {FIELDS} {JOINS}
         JOIN legal_nodes first ON first.version_id=n.version_id AND first.anchor=$2 AND first.kind='section' AND first.section IS NOT NULL
         JOIN legal_nodes last ON last.version_id=n.version_id AND last.anchor=$3 AND last.kind='section' AND last.section IS NOT NULL
         WHERE {DATE_FILTER} AND a.id=$4 AND n.kind='section' AND n.section IS NOT NULL AND n.position BETWEEN first.position AND last.position
         ORDER BY n.position LIMIT 21"
    );
    Ok(sqlx::query_as::<_, SourceRow>(&sql)
        .bind(date)
        .bind(start)
        .bind(end)
        .bind(act)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|row| row.0)
        .collect())
}

#[cfg(test)]
pub async fn search(
    pool: &PgPool,
    embeddings: Option<&Embeddings>,
    query: &str,
    date: NaiveDate,
    limit: i64,
) -> Result<Vec<Source>> {
    search_kind(pool, embeddings, query, date, limit, SourceKind::All).await
}

pub async fn search_kind(
    pool: &PgPool,
    embeddings: Option<&Embeddings>,
    query: &str,
    date: NaiveDate,
    limit: i64,
    kind: SourceKind,
) -> Result<Vec<Source>> {
    if unsupported_regulation_celex(query) {
        return Ok(Vec::new());
    }
    let references = if kind == SourceKind::Judgments {
        vec![]
    } else {
        references(query)?
    };
    if kind != SourceKind::Judgments && references.is_empty() {
        let regulations = whole_regulations(query);
        if !regulations.is_empty() {
            let mut results = Vec::new();
            for act in regulations {
                results.extend(get_provision(pool, "eu-regulation", date, Some(&act)).await?);
            }
            return Ok(results);
        }
    }
    let judgments = if kind == SourceKind::Legislation {
        None
    } else {
        crate::adapters::spi::db::judgments::references(pool, query)
            .await?
            .filter(|found| {
                kind == SourceKind::Judgments
                    || !found.is_empty()
                    || regulation_scopes(query).is_empty()
            })
    };
    if kind != SourceKind::Judgments
        && references.is_empty()
        && crate::adapters::spi::db::treaties::whole_document(query)
    {
        return get_provision(
            pool,
            "treaty",
            date,
            Some(crate::adapters::spi::db::treaties::ACT_ID),
        )
        .await;
    }
    if !references.is_empty() || judgments.is_some() {
        let mut results = Vec::new();
        for (act, anchor) in judgments.unwrap_or_default() {
            results.extend(get_provision(pool, &anchor, date, Some(&act)).await?);
        }
        for reference in references {
            // Never replace a nonexistent paragraph/letter with its parent.
            for source in
                get_provision(pool, &reference.anchor, date, reference.act.as_deref()).await?
            {
                if !results
                    .iter()
                    .any(|existing: &Source| existing.id == source.id)
                {
                    results.push(source);
                }
            }
        }
        results.retain(|source| kind.accepts(source));
        return Ok(results);
    }
    search_concepts_kind(pool, embeddings, query, date, limit, kind).await
}

pub async fn get_judgment(pool: &PgPool, query: &str, date: NaiveDate) -> Result<Vec<Source>> {
    let references = crate::adapters::spi::db::judgments::references(pool, query)
        .await?
        .unwrap_or_default();
    let mut acts: Vec<_> = references.into_iter().map(|(act, _)| act).collect();
    acts.sort();
    acts.dedup();
    ensure!(acts.len() == 1, "Specify exactly one imported judgment");
    let sql = format!(
        "SELECT {FIELDS} {JOINS} WHERE {DATE_FILTER} AND a.id=$2 AND a.judgment IS NOT NULL AND n.kind='judgment'"
    );
    let mut sources: Vec<Source> = sqlx::query_as::<_, SourceRow>(&sql)
        .bind(date)
        .bind(&acts[0])
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|row| row.0)
        .collect();
    for source in &mut sources {
        source.extend_retrieval_methods(&[RetrievalMethod::Exact]);
    }
    Ok(sources)
}

/// Search prose by its concepts even when it mentions a section number. Used
/// for reported gaps, whose citations may describe an insufficient source.
pub async fn search_concepts(
    pool: &PgPool,
    embeddings: Option<&Embeddings>,
    query: &str,
    date: NaiveDate,
    limit: i64,
) -> Result<Vec<Source>> {
    search_concepts_kind(pool, embeddings, query, date, limit, SourceKind::All).await
}

async fn search_concepts_kind(
    pool: &PgPool,
    embeddings: Option<&Embeddings>,
    query: &str,
    date: NaiveDate,
    limit: i64,
    kind: SourceKind,
) -> Result<Vec<Source>> {
    let regulations = regulation_scopes(query);
    if unsupported_regulation_celex(query)
        || (kind == SourceKind::Judgments && !regulations.is_empty())
    {
        return Ok(Vec::new());
    }
    let act = if kind == SourceKind::Judgments {
        None
    } else {
        act_scope(query)
    };
    // A concept query mentioning multiple acts needs separate scoped searches.
    // A failed scope must not broaden it to unrelated sources in the corpus.
    if !regulations.is_empty() && act.is_none() {
        return Ok(Vec::new());
    }
    let treaty_query;
    let query = if act.as_deref() == Some(crate::adapters::spi::db::treaties::ACT_ID) {
        treaty_query = crate::adapters::spi::db::treaties::concept_query(query);
        treaty_query.as_str()
    } else {
        query
    };
    let limit = limit.clamp(1, 12) as usize;
    let candidates = if embeddings.is_some() {
        48
    } else {
        limit as i64
    };
    let lexical = lexical_search(pool, query, date, candidates, act.as_deref(), kind);
    let semantic = async {
        match embeddings {
            Some(model) => semantic_search(pool, model, query, date, act.as_deref(), kind).await,
            None => Ok(Vec::new()),
        }
    };
    let (lexical, semantic) = tokio::join!(lexical, semantic);
    let lexical = lexical?;
    let semantic = match semantic {
        Ok(sources) => sources,
        Err(error) => {
            tracing::warn!(%error, "Semantic search unavailable; using lexical results");
            Vec::new()
        }
    };
    Ok(fuse(lexical, semantic, limit))
}

// Reciprocal Rank Fusion combines ranks, rather than incompatible IDF/cosine
// scores. A provision retrieved by both methods gets one citation and two votes.
fn fuse(lexical: Vec<Source>, semantic: Vec<Source>, limit: usize) -> Vec<Source> {
    let mut ranked: Vec<(Source, f64)> = Vec::new();
    for (sources, method) in [
        (lexical, RetrievalMethod::Fulltext),
        (semantic, RetrievalMethod::Embedding),
    ] {
        for (position, mut source) in sources.into_iter().enumerate() {
            source.extend_retrieval_methods(&[method]);
            let score = 1.0 / (60.0 + position as f64 + 1.0);
            if let Some((existing, total)) = ranked.iter_mut().find(|(s, _)| s.id == source.id) {
                existing.extend_retrieval_methods(&source.retrieval_methods);
                *total += score;
            } else {
                ranked.push((source, score));
            }
        }
    }
    // Stable sorting preserves lexical precedence on tied ranks.
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    ranked
        .into_iter()
        .take(limit)
        .map(|(source, _)| source)
        .collect()
}

async fn semantic_search(
    pool: &PgPool,
    model: &Embeddings,
    query: &str,
    date: NaiveDate,
    act: Option<&str>,
    kind: SourceKind,
) -> Result<Vec<Source>> {
    let kind_filter = kind.filter();
    // No provider request until this model has indexed eligible, current text.
    let sql = format!(
        "WITH inputs AS MATERIALIZED (SELECT node_id,content_hash FROM legal_embedding_inputs)
         SELECT m.dimensions {JOINS}
         JOIN inputs i ON i.node_id=n.id
         JOIN legal_embedding_documents d ON d.node_id=n.id AND d.content_hash=i.content_hash
         JOIN embedding_models m ON m.model_key=d.model_key
         WHERE {DATE_FILTER} AND {kind_filter} AND d.model_key=$2 AND ($3::text IS NULL OR a.id=$3) LIMIT 1"
    );
    let dimensions: Option<i32> = sqlx::query_scalar(&sql)
        .bind(date)
        .bind(model.key())
        .bind(act)
        .fetch_optional(pool)
        .await?;
    let Some(dimensions) = dimensions else {
        return Ok(Vec::new());
    };
    let vector = model.query_vector(query, dimensions).await?;
    let sql = format!(
        "WITH inputs AS MATERIALIZED (SELECT node_id,content_hash FROM legal_embedding_inputs),
        nearest AS MATERIALIZED (
            SELECT n.id, min(e.embedding <=> $2::text::vector) AS distance {JOINS}
            JOIN inputs i ON i.node_id=n.id
            JOIN legal_embedding_documents d ON d.node_id=n.id AND d.content_hash=i.content_hash
            JOIN legal_node_embeddings e ON e.model_key=d.model_key AND e.node_id=d.node_id
            WHERE {DATE_FILTER} AND {kind_filter} AND d.model_key=$3 AND ($4::text IS NULL OR a.id=$4)
            GROUP BY n.id
            HAVING min(e.embedding <=> $2::text::vector) <= 1.0-$5
            ORDER BY distance,n.id LIMIT 48
        ) SELECT {FIELDS} {JOINS} JOIN nearest ON nearest.id=n.id ORDER BY nearest.distance,n.id"
    );
    Ok(sqlx::query_as::<_, SourceRow>(&sql)
        .bind(date)
        .bind(vector)
        .bind(model.key())
        .bind(act)
        .bind(model.min_similarity())
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|row| row.0)
        .collect())
}

async fn lexical_search(
    pool: &PgPool,
    query: &str,
    date: NaiveDate,
    limit: i64,
    act: Option<&str>,
    kind: SourceKind,
) -> Result<Vec<Source>> {
    let kind_filter = kind.filter();
    let terms = query_terms(query);
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    // Score distinct query terms, weighted by inverse document frequency. Raw
    // ts_rank_cd over an OR query overvalues repeated generic legal vocabulary.
    let sql = format!(
        "WITH eligible AS MATERIALIZED (
            SELECT n.id,n.search_vector {JOINS} WHERE {DATE_FILTER} AND {kind_filter} AND n.searchable
                AND ($5::text IS NULL OR a.id=$5)
        ), q AS (
            SELECT to_tsquery('simple',lower(unaccent($2))) AS terms, lower(unaccent($3)) AS phrase
        ), terms AS (
            SELECT to_tsquery('simple',lower(unaccent(term))) AS term
            FROM unnest(string_to_array($2,' | ')) term
        ), weights AS MATERIALIZED (
            SELECT term, ln(1.0 + (SELECT count(*) FROM eligible)::float8 /
                (1.0 + (SELECT count(*) FROM eligible WHERE search_vector @@ term))) AS weight
            FROM terms
        )
        SELECT {FIELDS} {JOINS} CROSS JOIN q
        CROSS JOIN LATERAL (
            SELECT coalesce(sum(weight) FILTER(WHERE n.search_vector @@ term),0) AS score FROM weights
        ) rank
        WHERE {DATE_FILTER} AND {kind_filter} AND n.searchable
            AND ($5::text IS NULL OR a.id=$5)
            AND (n.search_vector @@ q.terms OR word_similarity(q.phrase,n.search_text)>0.35)
        ORDER BY (rank.score / power(1.0+length(n.own_text)/1000.0,0.2)
            + word_similarity(q.phrase,n.search_text)*0.2) DESC,n.position,n.id LIMIT $4"
    );
    Ok(sqlx::query_as::<_, SourceRow>(&sql)
        .bind(date)
        .bind(terms)
        .bind(query)
        .bind(limit)
        .bind(act)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|row| row.0)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multiple_references_keep_local_acts_and_suffixes() {
        let refs = references("§ 54 ods. 2 SP a § 362 ods. 1 CSP").unwrap();
        assert_eq!(
            refs,
            vec![
                Reference {
                    anchor: "paragraf-54.odsek-2".into(),
                    act: Some("SK/ZZ/1967/71".into())
                },
                Reference {
                    anchor: "paragraf-362.odsek-1".into(),
                    act: Some("SK/ZZ/2015/160".into())
                }
            ]
        );
        let refs = references("§ 49, 57 a 181 SSP").unwrap();
        assert_eq!(
            refs.iter().map(|r| r.anchor.as_str()).collect::<Vec<_>>(),
            vec!["paragraf-49", "paragraf-57", "paragraf-181"]
        );
        assert!(
            refs.iter()
                .all(|r| r.act.as_deref() == Some("SK/ZZ/2015/162"))
        );
        let refs = references("§ 181 ods. 1 a § 181 ods. 4 SSP").unwrap();
        assert_eq!(refs[0].anchor, "paragraf-181.odsek-1");
        assert_eq!(refs[1].anchor, "paragraf-181.odsek-4");
        assert_eq!(references("§ 181 SSP a § 181 SSP").unwrap().len(), 1);
        assert!(references("§ 181 až 183 SSP").is_err());
    }
    #[test]
    fn prefix_and_multiple_act_names_do_not_leak_other_acts() {
        let refs = references("CSP § 1 a CMP § 2").unwrap();
        assert_eq!(refs[0].act.as_deref(), Some("SK/ZZ/2015/160"));
        assert_eq!(refs[1].act.as_deref(), Some("SK/ZZ/2015/161"));
        assert_eq!(references("§ 1 CSP a CMP").unwrap().len(), 2);
        assert!(references("§ 181 ods. 1 a 4 SSP").is_err());
        assert!(!valid_act("nonsense"));
        assert!(!valid_act("CSP a CMP"));
    }
    #[test]
    fn articles_letters_and_unknown_laws_keep_identity() {
        let refs = references("čl. 2 ods. 1 CSP a § 1 písm. a) SSP").unwrap();
        assert_eq!(refs[0].anchor, "predpis.text2.clanok-2.odsek-1");
        assert_eq!(refs[1].anchor, "paragraf-1.odsek-1.pismeno-a");
        assert_eq!(
            references("§ 1 zákona č. 40/1964").unwrap()[0]
                .act
                .as_deref(),
            Some("SK/ZZ/1964/40")
        );
        assert!(references("odvolanie").unwrap().is_empty());
    }
    #[test]
    fn fusion_rewards_agreement_deduplicates_and_keeps_lexical_order_on_ties() {
        let source = |id| Source {
            id,
            anchor: String::new(),
            kind: String::new(),
            citation: String::new(),
            heading: String::new(),
            text: String::new(),
            context: String::new(),
            source_url: String::new(),
            pdf_url: String::new(),
            act_title: String::new(),
            valid_from: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            valid_to: None,
            verified_on: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            judgment: None,
            treaty: None,
            regulation: None,
            retrieval_methods: vec![],
        };
        let results = fuse(
            vec![source(1), source(2), source(3)],
            vec![source(3), source(4), source(1)],
            4,
        );
        assert_eq!(
            results.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![1, 3, 2, 4]
        );
        for source in &results[..2] {
            assert_eq!(
                source.retrieval_methods,
                vec![RetrievalMethod::Fulltext, RetrievalMethod::Embedding]
            );
        }
        assert_eq!(
            results[2].retrieval_methods,
            vec![RetrievalMethod::Fulltext]
        );
        assert_eq!(
            results[3].retrieval_methods,
            vec![RetrievalMethod::Embedding]
        );
        let lexical = fuse(vec![source(3), source(2), source(1)], vec![], 2);
        assert_eq!(lexical.iter().map(|s| s.id).collect::<Vec<_>>(), vec![3, 2]);
        assert!(
            lexical
                .iter()
                .all(|s| s.retrieval_methods == vec![RetrievalMethod::Fulltext])
        );

        let saved = serde_json::to_value(&results[0]).unwrap();
        assert_eq!(
            saved["retrieval_methods"],
            serde_json::json!(["fulltext", "embedding"])
        );
        let restored: Source = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(restored.retrieval_methods, results[0].retrieval_methods);
        let mut legacy = saved;
        legacy.as_object_mut().unwrap().remove("retrieval_methods");
        assert!(
            serde_json::from_value::<Source>(legacy)
                .unwrap()
                .retrieval_methods
                .is_empty()
        );
    }
    #[test]
    fn act_names_abbreviations_and_numbers_disambiguate_citations() {
        for query in [
            "§ 362 CSP",
            "Civilného sporového poriadku",
            "§ 1 zákona č. 160/2015 Z. z.",
        ] {
            assert_eq!(act_scope(query).as_deref(), Some("SK/ZZ/2015/160"));
        }
        assert_eq!(act_scope("čl. 2 cmp").as_deref(), Some("SK/ZZ/2015/161"));
        assert_eq!(
            act_scope("Civilný mimosporový poriadok").as_deref(),
            Some("SK/ZZ/2015/161")
        );
        assert_eq!(act_scope("§ 181 SSP").as_deref(), Some("SK/ZZ/2015/162"));
        assert_eq!(
            act_scope("Správneho súdneho poriadku").as_deref(),
            Some("SK/ZZ/2015/162")
        );
        assert_eq!(
            act_scope("§ 1 zákona č. 71/1967").as_deref(),
            Some("SK/ZZ/1967/71")
        );
        assert!(act_scope("§ 1").is_none());
        assert!(act_scope("Porovnaj CSP a CMP").is_none());
        for query in [
            "§ 54 SP",
            "Správny poriadok",
            "§ 54 ods. 2 Správneho poriadku",
            "§ 54 spravneho poriadku",
            "zákona o správnom konaní",
            "zakon o spravnom konani",
            "SP zákona č. 71/1967 Zb.",
        ] {
            assert_eq!(act_scope(query).as_deref(), Some("SK/ZZ/1967/71"));
        }
        assert!(act_scope("Porovnaj SP a SSP").is_none());
        for (queries, id) in [
            (
                [
                    "§ 2 ZoP",
                    "zákona o priestupkoch",
                    "Zákon Slovenskej národnej rady o priestupkoch",
                    "priestupkový zákon",
                ],
                "SK/ZZ/1990/372",
            ),
            (
                [
                    "§ 212 TZ",
                    "Trestného zákona",
                    "trestny zakon",
                    "zákona č. 300/2005 Z. z.",
                ],
                "SK/ZZ/2005/300",
            ),
            (
                [
                    "§ 34 TP",
                    "Trestného poriadku",
                    "trestny poriadok",
                    "zákona č. 301/2005 Z. z.",
                ],
                "SK/ZZ/2005/301",
            ),
        ] {
            for query in queries {
                assert_eq!(act_scope(query).as_deref(), Some(id), "{query}");
            }
        }
        assert!(act_scope("Porovnaj TZ a TP").is_none());
    }
    #[test]
    fn civil_code_aliases_and_mixed_citations_keep_their_acts() {
        for query in [
            "§ 420 OZ",
            "Občiansky zákonník",
            "§ 420 Občianskeho zákonníka",
            "§ 420 obcianskeho zakonnika",
            "§ 420 zákona č. 40/1964 Zb.",
        ] {
            assert_eq!(act_scope(query).as_deref(), Some("SK/ZZ/1964/40"));
            assert!(valid_act(query));
        }
        assert!(act_scope("Porovnaj OZ a CSP").is_none());
        let refs = references("§ 420 ods. 1 OZ a § 420 CSP").unwrap();
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].anchor, "paragraf-420.odsek-1");
        assert_eq!(refs[0].act.as_deref(), Some("SK/ZZ/1964/40"));
        assert_eq!(refs[1].anchor, "paragraf-420");
        assert_eq!(refs[1].act.as_deref(), Some("SK/ZZ/2015/160"));
    }
    #[test]
    fn consumer_act_aliases_do_not_override_other_laws_or_historical_numbers() {
        for query in [
            "§ 20 ZoS",
            "zákona o ochrane spotrebiteľa",
            "zakon o ochrane spotrebitela",
            "108/2024 Z. z.",
        ] {
            assert_eq!(act_scope(query).as_deref(), Some("SK/ZZ/2024/108"));
        }
        assert_eq!(
            act_scope("§ 3 zákona o ochrane spotrebiteľa č. 250/2007").as_deref(),
            Some("SK/ZZ/2007/250")
        );
        assert!(act_scope("ZoS a ZoP").is_none());
        let refs = references("§ 20 ods. 1 ZoS a § 619 OZ").unwrap();
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].act.as_deref(), Some("SK/ZZ/2024/108"));
        assert_eq!(refs[1].act.as_deref(), Some("SK/ZZ/1964/40"));
    }
    #[test]
    fn natural_language_query_produces_safe_prefix_terms() {
        assert_eq!(query_terms("Ako môžem podať žalobu?"), "podať:* | žalobu:*");
        assert!(!query_terms("'); DROP TABLE-- & <script>").contains('\''));
        assert!(query_terms("a na do").is_empty());
    }
    #[test]
    fn eu_regulation_identifiers_keep_jurisdiction_and_local_reference_scope() {
        for query in [
            "CELEX:32012R1215",
            "32012r1215",
            "nariadenie 1215/2012",
            "čl. 1 nariadenia (EÚ) č. 1215/2012",
            "čl. 1 nariadenia Európskeho parlamentu a Rady (EÚ) č. 1215/2012",
        ] {
            assert_eq!(
                act_scope(query).as_deref(),
                Some("EU/REG/32012R1215"),
                "{query}"
            );
        }
        for query in [
            "nariadenie (EÚ) 2016/679",
            "nariadenie 679/2016",
            "CELEX:32016R0679",
        ] {
            assert_eq!(
                act_scope(query).as_deref(),
                Some("EU/REG/32016R0679"),
                "{query}"
            );
        }
        assert_eq!(
            act_scope("čl. 1 nariadenia Rady (ES) č. 44/2001").as_deref(),
            Some("EU/REG/32001R0044")
        );
        assert_eq!(
            act_scope("nariadenie 2201/2003").as_deref(),
            Some("EU/REG/32003R2201")
        );
        assert!(regulation_scopes("nariadenie ۲۰۱۶/۶۷۹").is_empty());
        assert!(unsupported_regulation_celex("nariadenie 2016/2003"));
        assert_eq!(
            act_scope("§ 1 zákona č. 44/2001").as_deref(),
            Some("SK/ZZ/2001/44")
        );
        let refs = references("čl. 1 nariadenia 44/2001 a § 1 zákona č. 44/2001").unwrap();
        assert_eq!(refs.len(), 2);
        assert_eq!(
            refs[0],
            Reference {
                anchor: "predpis.text2.clanok-1".into(),
                act: Some("EU/REG/32001R0044".into())
            }
        );
        assert_eq!(
            refs[1],
            Reference {
                anchor: "paragraf-1".into(),
                act: Some("SK/ZZ/2001/44".into())
            }
        );
        let missing = references("čl. 1 ods. 999 CELEX:32012R1215").unwrap();
        assert_eq!(missing[0].anchor, "predpis.text2.clanok-1.odsek-999");
        assert_eq!(
            whole_regulations("CELEX:32012R1215"),
            vec!["EU/REG/32012R1215"]
        );
        assert_eq!(
            whole_regulations("nariadenie (EÚ) č. 1215/2012"),
            vec!["EU/REG/32012R1215"]
        );
        assert!(whole_regulations("právomoc podľa nariadenia 1215/2012").is_empty());
        assert!(whole_regulations("CELEX:41998A0126").is_empty());
        assert_eq!(
            act_scope("čl. 1 CELEX:32026R9999").as_deref(),
            Some("EU/REG/32026R9999")
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    #[ignore = "requires disposable PostgreSQL via DATABASE_URL"]
    async fn eu_regulation_search_preserves_identity_and_publication_boundaries(pool: PgPool) {
        use crate::application::repositories::CatalogRepository;
        let published = NaiveDate::from_ymd_opt(2001, 1, 16).unwrap();
        let checked = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        let metadata = crate::domain::Regulation {
            celex: "32001R0044".into(),
            language: "sk".into(),
            published_on: published,
            legal_context: "Pôvodné znenie, nie aktuálna konsolidácia.".into(),
        };
        sqlx::query("INSERT INTO acts(id,title,number,regulation) VALUES ('EU/REG/32001R0044','Testovacie nariadenie','44/2001',$1),('SK/ZZ/2001/44','Testovací zákon','44/2001',NULL)")
            .bind(sqlx::types::Json(&metadata)).execute(&pool).await.unwrap();
        let url = "https://eur-lex.europa.eu/legal-content/SK/TXT/HTML/?uri=CELEX:32001R0044";
        for act in ["EU/REG/32001R0044", "SK/ZZ/2001/44"] {
            let version: i64 = sqlx::query_scalar("INSERT INTO act_versions(act_id,valid_from,verified_on,source_url,pdf_url,sha256) VALUES($1,$2,$3,$4,'','fixture') RETURNING id")
                .bind(act).bind(published).bind(checked).bind(url).fetch_one(&pool).await.unwrap();
            for (anchor, kind, section) in [
                ("eu-regulation", "eu_regulation", None),
                ("predpis.text2.clanok-1", "article", None),
                ("paragraf-1", "section", Some("1")),
            ] {
                sqlx::query("INSERT INTO legal_nodes(version_id,anchor,kind,label,heading,citation,section,position,own_text,full_text,context_text,search_text,searchable) VALUES($1,$2,$3,'','', 'Čl. 1',$4,1,'fixture topic','fixture topic','fixture topic','fixture topic',true)")
                    .bind(version).bind(anchor).bind(kind).bind(section).execute(&pool).await.unwrap();
            }
        }
        for query in [
            "čl. 1 CELEX:32001R0044",
            "čl. 1 nariadenia Rady (ES) č. 44/2001",
        ] {
            let sources = search(&pool, None, query, checked, 8).await.unwrap();
            assert_eq!(sources.len(), 1, "{query}");
            assert_eq!(sources[0].regulation.as_ref().unwrap().celex, "32001R0044");
            assert_eq!(sources[0].source_url, url);
            assert_eq!(sources[0].citation, "Čl. 1 nariadenia EÚ č. 44/2001");
            assert_eq!(sources[0].valid_from, published);
            assert_eq!(sources[0].retrieval_methods, vec![RetrievalMethod::Exact]);
        }
        let mixed = search(
            &pool,
            None,
            "čl. 1 nariadenia 44/2001 a § 1 zákona č. 44/2001",
            checked,
            8,
        )
        .await
        .unwrap();
        assert_eq!(mixed.len(), 2);
        assert!(mixed[0].regulation.is_some());
        assert!(mixed[1].regulation.is_none());
        for query in ["CELEX:32001R0044", "32001R0044", "nariadenie 44/2001"] {
            let sources = search(&pool, None, query, checked, 8).await.unwrap();
            assert_eq!(sources.len(), 1, "{query}");
            assert_eq!(sources[0].anchor, "eu-regulation");
        }
        for query in [
            "CELEX:32001R9999",
            "čl. 1 CELEX:32001R9999",
            "čl. 1 ods. 999 CELEX:32001R0044",
            "čl. 1 CELEX:02001R0044-20150226",
            "čl. 1 CELEX:32001R0044(01)",
            "čl. 1 CELEX:32001R0044-20150226",
        ] {
            assert!(
                search(&pool, None, query, checked, 8)
                    .await
                    .unwrap()
                    .is_empty(),
                "{query}"
            );
        }
        let concept = search_kind(
            &pool,
            None,
            "fixture CELEX:32001R0044",
            checked,
            8,
            SourceKind::Legislation,
        )
        .await
        .unwrap();
        assert!(!concept.is_empty());
        assert!(concept.iter().all(|source| source.regulation.is_some()));
        assert!(
            search(
                &pool,
                None,
                "čl. 1 CELEX:32001R0044",
                published.pred_opt().unwrap(),
                8
            )
            .await
            .unwrap()
            .is_empty()
        );
        let after_verification = search(
            &pool,
            None,
            "čl. 1 CELEX:32001R0044",
            checked.succ_opt().unwrap(),
            8,
        )
        .await
        .unwrap();
        assert_eq!(after_verification.len(), 1);
        assert_eq!(after_verification[0].verified_on, checked);
        for query in ["CELEX:32001R0044", "fixture CELEX:32001R0044"] {
            assert!(
                search_kind(&pool, None, query, checked, 8, SourceKind::Judgments)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        assert!(
            search_concepts(
                &pool,
                None,
                "fixture nariadenia 44/2001 a nariadenia 1215/2012",
                checked,
                8
            )
            .await
            .unwrap()
            .is_empty()
        );
        let catalog = super::super::PgStore {
            pool,
            embeddings: None,
        }
        .corpus()
        .await
        .unwrap();
        assert!(catalog.versions.iter().any(|version| {
            version
                .regulation
                .as_ref()
                .is_some_and(|r| r.celex == "32001R0044")
        }));
    }
}
