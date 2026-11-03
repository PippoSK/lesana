//! Evidence policies run without JSON, an async runtime, or any I/O adapter.
use chrono::NaiveDate;
use lesana::domain::{Source, evidence::*};

fn source(id: i64) -> Source {
    let date = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
    Source {
        id,
        anchor: String::new(),
        kind: "section".into(),
        citation: "§ 181 SSP".into(),
        heading: String::new(),
        text: "Žaloba sa podáva v lehote dvoch mesiacov.".into(),
        context: String::new(),
        source_url: String::new(),
        pdf_url: String::new(),
        act_title: "SSP".into(),
        judgment: None,
        treaty: None,
        regulation: None,
        valid_from: date,
        valid_to: None,
        verified_on: date,
        retrieval_methods: vec![],
    }
}
fn claim(source: &Source) -> Claim {
    Claim {
        text: "Lehota je dva mesiace.".into(),
        evidence: vec![Evidence {
            source_id: source.id,
            quote: source.text.clone(),
        }],
    }
}
#[test]
fn provenance_is_checked_before_rendering_citations() {
    let first = source(1);
    let mut second = source(2);
    second.text = "Iné ustanovenie s celkom odlišným obsahom.".into();
    let mut forged = claim(&first);
    forged.evidence[0].source_id = second.id;
    assert!(validate(vec![forged], vec![], &[first.clone(), second.clone()]).is_err());
    let validated = validate(vec![claim(&first)], vec![], &[first, second]).unwrap();
    assert_eq!(validated.answer, "Lehota je dva mesiace. [N1]");
    assert_eq!(validated.sources.len(), 1);
}
#[test]
fn incomplete_reviews_never_authorize_a_partial_answer() {
    for verdicts in [vec![0], vec![0, 0], vec![0, 2]] {
        let review = Review {
            questions_safe: true,
            claims: verdicts
                .into_iter()
                .map(|index| Verdict {
                    index,
                    supported: true,
                    reason: String::new(),
                })
                .collect(),
        };
        let error = validate_review(review, 2).unwrap_err();
        assert!(error.downcast_ref::<ReviewFormatError>().is_some());
        assert!(error.downcast_ref::<ReviewRejected>().is_none());
    }
}
#[test]
fn partial_answer_rebuilds_sources_and_removes_unsafe_questions() {
    let sources = [source(1), source(2)];
    let draft = validate(
        sources.iter().map(claim).collect(),
        vec!["Kedy bolo doručené rozhodnutie?".into()],
        &sources,
    )
    .unwrap();
    let review = Review {
        questions_safe: false,
        claims: vec![
            Verdict {
                index: 0,
                supported: false,
                reason: "Chýba podmienka".into(),
            },
            Verdict {
                index: 1,
                supported: true,
                reason: String::new(),
            },
        ],
    };
    let error = validate_review(review, 2).unwrap_err();
    let partial = supported_part(&draft, error.downcast_ref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        partial.sources.iter().map(|s| s.id).collect::<Vec<_>>(),
        vec![2]
    );
    assert_eq!(partial.claims.len(), 1);
    assert!(partial.questions.is_empty());
    assert!(!partial.answer.contains("[N1]"));
}
#[test]
fn empty_answers_unattributed_claims_and_injected_citations_are_rejected() {
    assert!(validate(vec![], vec![], &[]).is_err());
    let source = source(1);
    let mut input = claim(&source);
    input.text.push_str(" [N99]");
    assert!(validate(vec![input], vec![], std::slice::from_ref(&source)).is_err());
    assert!(
        validate(
            vec![Claim {
                text: "Bez dôkazu".into(),
                evidence: vec![]
            }],
            vec![],
            &[source]
        )
        .is_err()
    );
}
