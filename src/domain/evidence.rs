use crate::domain::Source;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub source_id: i64,
    pub quote: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    pub text: String,
    pub evidence: Vec<Evidence>,
}

#[derive(Serialize)]
pub struct Passage {
    pub id: usize,
    pub quote: String,
}

#[derive(Serialize)]
pub struct SourcePassages {
    pub source_id: i64,
    pub citation: String,
    pub passages: Vec<Passage>,
}

pub fn passages(source: &Source) -> Vec<Passage> {
    let words = regex::Regex::new(r"\S+").unwrap();
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for text in [&source.text, &source.context] {
        let spans: Vec<_> = words.find_iter(text).collect();
        let mut start = 0;
        while start < spans.len() {
            let mut end = start + 1;
            while end < spans.len() && spans[end].end() - spans[start].start() <= 1800 {
                end += 1;
            }
            let quote = &text[spans[start].start()..spans[end - 1].end()];
            if (20..=2500).contains(&quote.chars().count()) && seen.insert(normalized(quote)) {
                result.push(Passage {
                    id: result.len(),
                    quote: quote.to_owned(),
                });
            }
            if end == spans.len() {
                break;
            }
            // Retain a little context across long-paragraph boundaries, without
            // truncating a word or changing the original whitespace/text.
            let mut next = end;
            while next > start + 1 && spans[end - 1].end() - spans[next - 1].start() <= 160 {
                next -= 1;
            }
            start = next;
        }
    }
    result
}

pub fn catalog(sources: &[Source]) -> Vec<SourcePassages> {
    sources
        .iter()
        .map(|source| SourcePassages {
            source_id: source.id,
            citation: source.citation.clone(),
            passages: passages(source),
        })
        .collect()
}

#[derive(Debug)]
pub struct ReviewRejected {
    pub details: Vec<RejectedClaim>,
    pub questions_safe: bool,
}
impl std::fmt::Display for ReviewRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let indices: Vec<_> = self.details.iter().map(|item| item.index).collect();
        write!(
            f,
            "Claims not supported by their evidence: {indices:?}; questions_safe={}",
            self.questions_safe
        )
    }
}
impl std::error::Error for ReviewRejected {}

#[derive(Debug)]
pub struct ReviewFormatError {
    pub detail: String,
}
impl std::fmt::Display for ReviewFormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Invalid evidence review format")
    }
}
impl std::error::Error for ReviewFormatError {}

pub struct Validated {
    pub answer: String,
    pub sources: Vec<Source>,
    pub claims: Vec<Claim>,
    pub questions: Vec<String>,
}

fn normalized(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// This is a provenance check, not a proof of legal entailment. Semantic review
// runs separately, and is itself fallible. Do not label this legal certification.
pub fn validate(
    claims: Vec<Claim>,
    questions: Vec<String>,
    sources: &[Source],
) -> Result<Validated> {
    ensure!(
        claims.len() <= 20 && questions.len() <= 4,
        "Too many claims or questions"
    );
    ensure!(!claims.is_empty() || !questions.is_empty(), "Empty answer");
    let mut cited = HashSet::new();
    let mut paragraphs = Vec::new();
    for (index, claim) in claims.iter().enumerate() {
        ensure!(
            !claim.text.trim().is_empty()
                && claim.text.chars().count() <= 1800
                && !claim.text.contains("[N"),
            "Invalid text of claim {index}"
        );
        ensure!(
            !claim.evidence.is_empty() && claim.evidence.len() <= 6,
            "Claim {index} needs evidence"
        );
        let mut ids = Vec::new();
        for evidence in &claim.evidence {
            let source = sources
                .iter()
                .find(|s| s.id == evidence.source_id)
                .with_context(|| format!("Unavailable source for claim {index}"))?;
            let quote = normalized(&evidence.quote);
            ensure!(
                (20..=2500).contains(&quote.chars().count()),
                "Quote length invalid for claim {index}"
            );
            ensure!(
                normalized(&source.text).contains(&quote)
                    || normalized(&source.context).contains(&quote),
                "Quote not found in source for claim {index}"
            );
            cited.insert(source.id);
            if !ids.contains(&source.id) {
                ids.push(source.id);
            }
        }
        paragraphs.push(format!(
            "{} {}",
            claim.text.trim(),
            ids.iter()
                .map(|id| format!("[N{id}]"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    for question in &questions {
        ensure!(
            question.trim().ends_with('?')
                && question.chars().count() <= 500
                && !question.contains("[N"),
            "Invalid follow-up question"
        );
        paragraphs.push(question.trim().to_owned());
    }
    Ok(Validated {
        answer: paragraphs.join("\n\n"),
        sources: sources
            .iter()
            .filter(|s| cited.contains(&s.id))
            .cloned()
            .collect(),
        claims,
        questions,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Review {
    pub claims: Vec<Verdict>,
    pub questions_safe: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verdict {
    pub index: usize,
    pub supported: bool,
    #[serde(default)]
    pub reason: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct RejectedClaim {
    pub index: usize,
    pub reason: String,
}
pub fn validate_review(review: Review, claim_count: usize) -> Result<()> {
    if review.claims.len() != claim_count {
        return Err(ReviewFormatError {
            detail: format!(
                "Review must contain exactly {claim_count} claim verdicts, received {}",
                review.claims.len()
            ),
        }
        .into());
    }
    let mut seen = HashSet::new();
    let mut rejected = Vec::new();
    for verdict in review.claims {
        if verdict.index >= claim_count || !seen.insert(verdict.index) {
            return Err(ReviewFormatError {
                detail: format!(
                    "Invalid or duplicate review index {} for {claim_count} claims",
                    verdict.index
                ),
            }
            .into());
        }
        if !verdict.supported {
            rejected.push(RejectedClaim {
                index: verdict.index,
                reason: verdict.reason.chars().take(500).collect(),
            });
        }
    }
    if !rejected.is_empty() || !review.questions_safe {
        return Err(ReviewRejected {
            details: rejected,
            questions_safe: review.questions_safe,
        }
        .into());
    }
    Ok(())
}

// ReviewRejected is only emitted after a complete, well-formed review.
// Every retained claim therefore has an explicit true verdict
// based on its own evidence; missing/malformed verdicts never reach this path.
pub fn supported_part(draft: &Validated, rejected: &ReviewRejected) -> Result<Option<Validated>> {
    let excluded: HashSet<_> = rejected.details.iter().map(|item| item.index).collect();
    let claims: Vec<_> = draft
        .claims
        .iter()
        .enumerate()
        .filter(|(index, _)| !excluded.contains(index))
        .map(|(_, claim)| claim.clone())
        .collect();
    if claims.is_empty() {
        return Ok(None);
    }
    // Rebuild citations and the source list from the retained claims only.
    validate(
        claims,
        if rejected.questions_safe {
            draft.questions.clone()
        } else {
            vec![]
        },
        &draft.sources,
    )
    .map(Some)
}
