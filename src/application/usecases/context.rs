//! Build a bounded evidence context independently of the model's tool choices.
use crate::application::repositories::LegalRepository;
use crate::domain::{ReferenceLink, RetrievalMethod, Source};
use anyhow::Result;
use chrono::NaiveDate;
use regex::Regex;
use serde::Serialize;
use std::collections::HashMap;

const MAX_SOURCES: usize = 60;
const MAX_BYTES: usize = 180_000;
// Leave most of the budget for direct retrieval and its mandatory parents.
const MAX_LINK_BYTES: usize = 60_000;
const LINK_DEPTH: usize = 2;
const MAX_LINKS: usize = 40;

#[derive(Clone, Debug, Serialize)]
pub struct ContextIssue {
    pub as_of: NaiveDate,
    pub source_id: i64,
    pub target: String,
    pub reason: &'static str,
}

#[derive(Default)]
pub struct EvidenceContext {
    pub sources: Vec<Source>,
    pub issues: Vec<ContextIssue>,
    expanded: HashMap<(i64, NaiveDate), usize>,
    link_bytes: usize,
}

fn size(source: &Source) -> usize {
    source.text.len() + source.context.len()
}

impl EvidenceContext {
    fn issue(&mut self, source_id: i64, target: String, reason: &'static str, date: NaiveDate) {
        if self.issues.len() >= 80 {
            if self.issues.len() == 80 {
                self.issues.push(ContextIssue {
                    as_of: date,
                    source_id,
                    target: String::new(),
                    reason: "issue_count_limit",
                });
            }
            return;
        }
        if !self.issues.iter().any(|i| {
            i.source_id == source_id && i.target == target && i.reason == reason && i.as_of == date
        }) {
            self.issues.push(ContextIssue {
                as_of: date,
                source_id,
                target,
                reason,
            });
        }
    }

    fn insert(
        &mut self,
        mut source: Source,
        related: bool,
        linked: bool,
        date: NaiveDate,
        returned: &mut Vec<Source>,
    ) -> bool {
        if related {
            source.retrieval_methods = vec![RetrievalMethod::Related];
        }
        if let Some(existing) = self.sources.iter_mut().find(|s| s.id == source.id) {
            existing.extend_retrieval_methods(&source.retrieval_methods);
            if let Some(item) = returned.iter_mut().find(|s| s.id == source.id) {
                *item = existing.clone();
            } else {
                returned.push(existing.clone());
            }
            return true;
        }
        let bytes = size(&source);
        if self.sources.len() >= MAX_SOURCES
            || self.sources.iter().map(size).sum::<usize>() + bytes > MAX_BYTES
            || (linked && self.link_bytes + bytes > MAX_LINK_BYTES)
        {
            self.issue(
                source.id,
                source.citation.clone(),
                "context_budget_exceeded",
                date,
            );
            return false;
        }
        if linked {
            self.link_bytes += bytes;
        }
        self.sources.push(source.clone());
        returned.push(source);
        true
    }

    /// Used for initial results, conversation follow-ups, and every tool result.
    pub async fn add(
        &mut self,
        repository: &dyn LegalRepository,
        new: Vec<Source>,
        date: NaiveDate,
    ) -> Result<Vec<Source>> {
        let mut returned = Vec::new();
        let mut frontier = Vec::new();
        // Complete all direct hits before allowing outgoing links to use budget.
        for source in new {
            let parent = repository.containing_provision(source.id, date).await?;
            let original_id = source.id;
            if self.insert(source.clone(), false, false, date, &mut returned) {
                let root = parent.unwrap_or(source);
                let related = root.id != original_id;
                if self.insert(root.clone(), related, false, date, &mut returned) {
                    frontier.push(root);
                }
            }
        }
        // Two link hops include references reached through footnotes. Cycles and
        // repeated tool calls never cause recursive, unbounded corpus loading.
        for depth in 0..=LINK_DEPTH {
            let mut next = Vec::new();
            for source in frontier {
                let remaining = LINK_DEPTH - depth;
                if self
                    .expanded
                    .get(&(source.id, date))
                    .is_some_and(|old| *old >= remaining)
                {
                    continue;
                }
                self.expanded.insert((source.id, date), remaining);
                let links = repository.reference_links(source.id).await?;
                if links.len() > MAX_LINKS {
                    self.issue(
                        source.id,
                        source.citation.clone(),
                        "reference_count_limit",
                        date,
                    );
                }
                for link in links.into_iter().take(MAX_LINKS) {
                    let targets = match targets(&source.source_url, &link) {
                        Ok(targets) => targets,
                        Err(reason) => {
                            self.issue(source.id, link.href, reason, date);
                            continue;
                        }
                    };
                    for target in targets {
                        let found = if let Some(end) = &target.end {
                            repository
                                .section_range(&target.anchor, end, &target.act, date)
                                .await?
                        } else {
                            repository
                                .get_provision(&target.anchor, date, Some(&target.act))
                                .await?
                        };
                        if found.is_empty() {
                            self.issue(
                                source.id,
                                target.description(),
                                "unavailable_at_selected_date",
                                date,
                            );
                            continue;
                        }
                        if found.len() > 20 {
                            self.issue(
                                source.id,
                                target.description(),
                                "reference_count_limit",
                                date,
                            );
                            continue;
                        }
                        for found in found {
                            let individual = Target {
                                act: target.act.clone(),
                                anchor: found.anchor.clone(),
                                end: None,
                            };
                            if self.sources.iter().any(|s| {
                                s.valid_from == found.valid_from && covers(s, &individual, date)
                            }) {
                                continue;
                            }
                            if remaining == 0 {
                                self.issue(
                                    source.id,
                                    individual.description(),
                                    "reference_depth_limit",
                                    date,
                                );
                                continue;
                            }
                            // Verify the exact target before completing its parent.
                            let complete = repository
                                .containing_provision(found.id, date)
                                .await?
                                .unwrap_or(found);
                            if self.insert(complete.clone(), true, true, date, &mut returned) {
                                next.push(complete);
                            }
                        }
                    }
                }
            }
            frontier = next;
        }
        // A later explicit tool call may resolve a gap left by the bounded walk.
        self.issues.retain(|issue| {
            if issue.reason == "context_budget_exceeded" {
                return !self.sources.iter().any(|s| s.id == issue.source_id);
            }
            if issue.reason == "reference_depth_limit"
                && let Some((act, anchor)) = issue.target.split_once('#')
            {
                let target = Target {
                    act: act.to_owned(),
                    anchor: anchor.to_owned(),
                    end: None,
                };
                return !self.sources.iter().any(|s| covers(s, &target, issue.as_of));
            }
            true
        });
        Ok(returned)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Target {
    act: String,
    anchor: String,
    end: Option<String>,
}
impl Target {
    fn description(&self) -> String {
        format!(
            "{}#{}{}",
            self.act,
            self.anchor,
            self.end
                .as_ref()
                .map(|end| format!(" až {end}"))
                .unwrap_or_default()
        )
    }
}

fn act_from_url(url: &url::Url) -> Option<String> {
    let pattern = Regex::new(r"/SK/ZZ/(\d{4})/(\d{1,4})/").unwrap();
    let captures = pattern.captures(url.path())?;
    Some(format!("SK/ZZ/{}/{}", &captures[1], &captures[2]))
}

fn covers(source: &Source, target: &Target, date: NaiveDate) -> bool {
    if source.valid_from > date || source.valid_to.is_some_and(|to| date > to) {
        return false;
    }
    let Ok(url) = url::Url::parse(&source.source_url) else {
        return false;
    };
    act_from_url(&url).as_deref() == Some(&target.act)
        && (source.anchor == target.anchor
            || (matches!(source.kind.as_str(), "section" | "article")
                && target.anchor.starts_with(&format!("{}.", source.anchor))))
}

// Only official hyperlink identities are followed. No network requests, fuzzy
// law matching, or interpretation of a bare section number in prose.
fn targets(base: &str, link: &ReferenceLink) -> std::result::Result<Vec<Target>, &'static str> {
    let url = url::Url::parse(base)
        .and_then(|base| base.join(&link.href))
        .map_err(|_| "unsupported_reference")?;
    if !matches!(url.scheme(), "https" | "http")
        || !matches!(
            url.host_str(),
            Some("static.slov-lex.sk" | "www.slov-lex.sk" | "slov-lex.sk")
        )
    {
        return Err("unsupported_reference");
    }
    let act = act_from_url(&url).ok_or("unsupported_reference")?;
    let anchor = url
        .fragment()
        .filter(|s| !s.is_empty())
        .ok_or("whole_act_reference_requires_search")?;
    if !anchor.starts_with("paragraf-")
        && !anchor.starts_with("predpis.text2.clanok-")
        && !anchor.starts_with("poznamky.poznamka-")
    {
        return Err("unsupported_reference");
    }
    // Slov-Lex sometimes links a section range to its first section only.
    let range =
        Regex::new(r"^§{1,2}\s*(\d{1,4}[a-z]{0,2})\s*(?:až|–|-)\s*(\d{1,4}[a-z]{0,2})\b").unwrap();
    if let Some(c) = range.captures(&link.label) {
        if anchor != format!("paragraf-{}", &c[1]) {
            return Err("unsupported_reference_range");
        }
        return Ok(vec![Target {
            act,
            anchor: anchor.to_owned(),
            end: Some(format!("paragraf-{}", &c[2])),
        }]);
    }
    let mut result = vec![Target {
        act: act.clone(),
        anchor: anchor.to_owned(),
        end: None,
    }];
    // Likewise a link may label a list such as “§ 336c a 336d”. Paragraph
    // and letter lists are already covered by completing their one section.
    let first = Regex::new(r"^§{1,2}\s*(\d{1,4}[a-z]{0,2})\b").unwrap();
    let item = Regex::new(r"^\s*(?:,\s*(?:a\s+)?|a\s+)(?:§\s*)?(\d{1,4}[a-z]{0,2})\b").unwrap();
    if let Some(c) = first.captures(&link.label)
        && anchor == format!("paragraf-{}", &c[1])
    {
        let mut tail = &link.label[c.get(0).unwrap().end()..];
        while let Some(c) = item.captures(tail) {
            if result.len() >= 20 {
                return Err("unsupported_reference_range");
            }
            result.push(Target {
                act: act.clone(),
                anchor: format!("paragraf-{}", &c[1]),
                end: None,
            });
            tail = &tail[c.get(0).unwrap().end()..];
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    const BASE: &str =
        "https://static.slov-lex.sk/static/SK/ZZ/2005/300/20260818.html#paragraf-216";

    #[test]
    fn links_keep_law_identity_and_expand_bounded_section_ranges() {
        let link = |href: &str, label: &str| ReferenceLink {
            href: href.into(),
            label: label.into(),
        };
        let local = targets(BASE, &link("#paragraf-125.odsek-1", "§ 125 ods. 1")).unwrap();
        assert_eq!(
            local,
            vec![Target {
                act: "SK/ZZ/2005/300".into(),
                anchor: "paragraf-125.odsek-1".into(),
                end: None,
            }]
        );
        let external = targets(
            BASE,
            &link("../../../../SK/ZZ/2005/301/#paragraf-216", "§ 216"),
        )
        .unwrap();
        assert_eq!(external[0].act, "SK/ZZ/2005/301");
        let range = targets(BASE, &link("#paragraf-5", "§ 5 až 7")).unwrap();
        assert_eq!(range[0].anchor, "paragraf-5");
        assert_eq!(range[0].end.as_deref(), Some("paragraf-7"));
        assert_eq!(
            targets(
                BASE,
                &link("#paragraf-328", "§ 328 až 330 Trestného zákona")
            )
            .unwrap()[0]
                .end
                .as_deref(),
            Some("paragraf-330")
        );
        assert_eq!(
            targets(
                BASE,
                &link("#paragraf-336c", "§ 336c a 336d Trestného zákona")
            )
            .unwrap()[1]
                .anchor,
            "paragraf-336d"
        );
        assert!(
            targets(
                BASE,
                &link("https://other.example/SK/ZZ/2005/300/#paragraf-1", "§ 1")
            )
            .is_err()
        );
        assert_eq!(
            targets(BASE, &link("../../../../SK/ZZ/1964/40/", "40/1964")),
            Err("whole_act_reference_requires_search")
        );
        assert!(targets(BASE, &link("#poznamky.poznamka-1", "1)")).is_ok());
    }

    fn source(id: i64) -> Source {
        serde_json::from_value(serde_json::json!({
            "id":id,"anchor":"paragraf-216","kind":"section","citation":"§ 216 TZ",
            "heading":"","text":"Znenie ustanovenia.","context":"Znenie ustanovenia.",
            "source_url":BASE,"pdf_url":"","act_title":"Trestný zákon",
            "valid_from":"2026-08-18","valid_to":null,"verified_on":"2026-10-01",
            "retrieval_methods":["fulltext"]
        }))
        .unwrap()
    }

    #[test]
    fn budgets_are_explicit_and_duplicates_preserve_provenance() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let mut context = EvidenceContext::default();
        let mut returned = Vec::new();
        assert!(context.insert(source(1), false, false, date, &mut returned));
        let mut repeated = source(1);
        repeated.retrieval_methods = vec![RetrievalMethod::Embedding];
        assert!(context.insert(repeated, false, false, date, &mut returned));
        assert!(context.insert(source(1), true, false, date, &mut returned));
        assert_eq!(context.sources.len(), 1);
        assert_eq!(returned.len(), 1);
        assert_eq!(
            context.sources[0].retrieval_methods,
            vec![
                RetrievalMethod::Fulltext,
                RetrievalMethod::Embedding,
                RetrievalMethod::Related
            ]
        );
        let mut large = source(2);
        large.text = "x".repeat(MAX_LINK_BYTES);
        assert!(!context.insert(large.clone(), true, true, date, &mut returned));
        assert_eq!(context.issues[0].reason, "context_budget_exceeded");
        // A direct tool request can still use capacity reserved from link walks.
        assert!(context.insert(large, false, false, date, &mut returned));
        let mut huge = source(3);
        huge.text = "x".repeat(MAX_BYTES);
        assert!(!context.insert(huge, false, false, date, &mut returned));
        assert!(!covers(
            &source(1),
            &Target {
                act: "SK/ZZ/2005/301".into(),
                anchor: "paragraf-216.odsek-1".into(),
                end: None
            },
            date
        ));
    }
}
