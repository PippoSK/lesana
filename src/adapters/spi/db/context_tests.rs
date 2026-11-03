use super::search;
use crate::application::usecases::context::EvidenceContext;
use crate::domain::RetrievalMethod;
use chrono::NaiveDate;
use sqlx::PgPool;
const MAX_SOURCES: usize = 60;

// Synthetic statutes deliberately contain different text in each version.
// Exercise the real SQL filters and context expansion, not a date-blind mock.
#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL via DATABASE_URL"]
async fn historical_tools_keep_versions_parents_links_and_gaps_at_the_requested_date(pool: PgPool) {
    use crate::application::{
        contracts::{ResearchRequest, ResearchResult},
        repositories::ProgressSink,
        usecases::retrieval::Session,
        utils::events::{Events, ProgressEvent},
    };
    use std::sync::Arc;
    struct Quiet;
    impl ProgressSink for Quiet {
        fn emit(
            &self,
            _: ProgressEvent,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            Box::pin(async {})
        }
    }
    let parse = |date: &str| NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap();
    let selected = parse("2026-10-13");
    let verified = parse("2026-10-01");
    let historical = parse("2023-09-29");
    for (act, from, to, section) in [
        ("2001/566", "2023-06-01", Some("2023-12-31"), "30"),
        ("2001/566", "2026-06-25", None, "30"),
        ("1991/513", "2023-01-01", Some("2023-12-31"), "409"),
        ("1991/513", "2026-01-01", None, "409"),
        ("1964/40", "2026-01-01", None, "52"),
        ("1967/71", "2018-09-01", None, "1"),
    ] {
        let (year, number) = act.split_once('/').unwrap();
        let act_id = format!("SK/ZZ/{act}");
        sqlx::query("INSERT INTO acts(id,title,number) VALUES($1,'Testovací predpis',$2) ON CONFLICT DO NOTHING")
            .bind(&act_id).bind(format!("{number}/{year}")).execute(&pool).await.unwrap();
        let version: i64 = sqlx::query_scalar("INSERT INTO act_versions(act_id,valid_from,valid_to,verified_on,source_url,pdf_url,sha256) VALUES($1,$2,$3,$4,$5,'','test') RETURNING id")
            .bind(&act_id).bind(parse(from)).bind(to.map(parse)).bind(verified)
            .bind(format!("https://static.slov-lex.sk/static/SK/ZZ/{act}/{}.html", from.replace('-', "")))
            .fetch_one(&pool).await.unwrap();
        let links = if matches!(act, "2001/566" | "1967/71") {
            serde_json::json!([
                {"href":"https://static.slov-lex.sk/static/SK/ZZ/1991/513/#paragraf-409","label":"§ 409"},
                {"href":"https://static.slov-lex.sk/static/SK/ZZ/1964/40/#paragraf-52","label":"§ 52"}
            ])
        } else {
            serde_json::json!([])
        };
        for child in [false, true] {
            let parent = format!("paragraf-{section}");
            let anchor = if child {
                format!("{parent}.odsek-1")
            } else {
                parent.clone()
            };
            sqlx::query("INSERT INTO legal_nodes(version_id,anchor,parent_anchor,kind,label,heading,citation,section,position,own_text,full_text,context_text,search_text,searchable,reference_links) VALUES($1,$2,$3,$4,'','',$5,$6,1,$7,$7,'',$7,true,$8)")
                .bind(version).bind(anchor).bind(child.then_some(parent))
                .bind(if child {"paragraph"} else {"section"}).bind(format!("§ {section}"))
                .bind(section).bind(format!("Testovací text {act} od {from}."))
                .bind(if child {serde_json::json!([])} else {links.clone()})
                .execute(&pool).await.unwrap();
        }
    }
    let repository = Arc::new(super::PgLegalRepository::new(pool, None));
    let events: Events = Arc::new(Quiet);
    let session = Session::new(repository, selected, &events);
    // First load today's linked provisions. They must not suppress older ones.
    session
        .execute(ResearchRequest::GetSection {
            section: "1".into(),
            act: "71/1967".into(),
            as_of: None,
        })
        .await
        .unwrap();
    let result = session
        .execute(ResearchRequest::SearchNorms {
            query: "§ 30 ods. 1 566/2001".into(),
            as_of: Some(historical),
        })
        .await
        .unwrap();
    let ResearchResult::Sources {
        as_of,
        sources,
        context_issues,
    } = result
    else {
        panic!("valid historical lookup rejected")
    };
    assert_eq!(as_of, historical);
    assert_eq!(sources.len(), 3); // Exact paragraph, its parent, historical link.
    assert!(
        sources
            .iter()
            .all(|s| s.valid_from <= historical && s.valid_to.is_some_and(|to| historical <= to))
    );
    assert!(
        sources
            .iter()
            .any(|s| s.anchor == "paragraf-30" && s.valid_from == parse("2023-06-01"))
    );
    assert!(
        sources
            .iter()
            .any(|s| s.anchor == "paragraf-409" && s.valid_from == parse("2023-01-01"))
    );
    assert!(
        context_issues
            .iter()
            .any(|i| i.reason == "unavailable_at_selected_date"
                && i.as_of == historical
                && i.target == "SK/ZZ/1964/40#paragraf-52")
    );
    // The same long-lived root must expand its links again at the older date.
    let result = session
        .execute(ResearchRequest::GetSection {
            section: "1".into(),
            act: "71/1967".into(),
            as_of: Some(historical),
        })
        .await
        .unwrap();
    let ResearchResult::Sources {
        sources,
        context_issues,
        ..
    } = result
    else {
        panic!()
    };
    let root = sources.iter().find(|s| s.anchor == "paragraf-1").unwrap();
    assert!(context_issues.iter().any(|i| i.source_id == root.id
        && i.as_of == historical
        && i.reason == "unavailable_at_selected_date"));
    // A historical override never changes the default of later calls.
    let result = session
        .execute(ResearchRequest::GetSection {
            section: "30".into(),
            act: "566/2001".into(),
            as_of: None,
        })
        .await
        .unwrap();
    let ResearchResult::Sources { as_of, sources, .. } = result else {
        panic!()
    };
    assert_eq!(as_of, selected);
    assert!(sources.iter().all(|source| source.verified_on == verified));
    assert!(
        sources
            .iter()
            .any(|s| s.anchor == "paragraf-30" && s.valid_from == parse("2026-06-25"))
    );
    // Missing periods stay empty rather than substituting the latest import.
    for date in ["2023-05-31", "2024-01-01"] {
        let result = session
            .execute(ResearchRequest::GetSection {
                section: "30".into(),
                act: "566/2001".into(),
                as_of: Some(parse(date)),
            })
            .await
            .unwrap();
        let ResearchResult::Sources { as_of, sources, .. } = result else {
            panic!()
        };
        assert_eq!(as_of, parse(date));
        assert!(sources.is_empty());
    }
    assert!(matches!(
        session
            .execute(ResearchRequest::SearchNorms {
                query: "§ 30 566/2001".into(),
                as_of: Some(parse("2027-01-01"))
            })
            .await
            .unwrap(),
        ResearchResult::InvalidArguments
    ));
    assert_eq!(session.calls().await[1].as_of, Some(historical));
    let context = session.take_context().await;
    assert_eq!(
        context
            .sources
            .iter()
            .filter(|s| s.anchor == "paragraf-409")
            .count(),
        2
    );
}

// sqlx creates and drops a separate database; never run against app tables.
#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL via DATABASE_URL"]
async fn complete_context_and_links_obey_identity_date_and_limits(pool: PgPool) {
    for (html, manifest) in crate::adapters::spi::db::import::BUNDLED_CORPORA {
        let manifest = serde_json::from_str(manifest).unwrap();
        crate::adapters::spi::db::import::import(&pool, html, &manifest)
            .await
            .unwrap();
    }
    let repository = super::PgLegalRepository::new(pool.clone(), None);
    let date = NaiveDate::from_ymd_opt(2026, 10, 13).unwrap();
    // Quoted amendments in 108/2024 must not masquerade as its own § 619.
    assert!(
        search::get_section(&pool, "619", date, Some("ZoS"))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        search::get_section(&pool, "619", date, Some("OZ"))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        search::section_range(
            &pool,
            "paragraf-612",
            "paragraf-626",
            "SK/ZZ/2024/108",
            date
        )
        .await
        .unwrap()
        .is_empty()
    );
    let mut context = EvidenceContext::default();
    let hits = search::search(&pool, None, "§ 216 ods. 2 písm. c) TZ", date, 6)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1); // Public precise search stays precise.
    let added = context.add(&repository, hits.clone(), date).await.unwrap();
    assert_eq!(added.len(), 2);
    let parent = context
        .sources
        .iter()
        .find(|s| s.anchor == "paragraf-216")
        .unwrap();
    assert_eq!(parent.act_title, "Trestný zákon");
    assert!(parent.text.contains("v úmysle prechodne ho používať"));
    assert_eq!(parent.retrieval_methods, [RetrievalMethod::Related]);
    let catalog = crate::domain::evidence::catalog(&context.sources);
    assert!(
        catalog
            .iter()
            .find(|s| s.source_id == parent.id)
            .unwrap()
            .passages
            .iter()
            .any(|p| p.quote.contains("v úmysle prechodne ho používať"))
    );
    context.add(&repository, hits, date).await.unwrap();
    assert_eq!(context.sources.len(), 2);
    assert!(context.issues.is_empty());

    let concepts = search::search_concepts(
        &pool,
        None,
        "Škodou malou sa rozumie škoda. Definícia chýba v § 115 TZ.",
        date,
        6,
    )
    .await
    .unwrap();
    assert!(
        concepts
            .iter()
            .any(|s| s.anchor.starts_with("paragraf-125."))
    );
    assert!(
        concepts
            .iter()
            .all(|s| !s.retrieval_methods.contains(&RetrievalMethod::Exact))
    );

    for (query, anchor, law) in [
        ("§ 62 ods. 1 písm. d) SP", "paragraf-9", "71/1967"),
        ("§ 9 ods. 2 TP", "paragraf-326a", "300/2005"),
        ("§ 19 ods. 4 SP", "paragraf-7", "71/1967"),
        ("§ 55 ods. 1 ZoP", "paragraf-7", "71/1967"),
        ("čl. 2 ods. 1 CSP", "predpis.text2.clanok-2", "160/2015"),
    ] {
        let mut context = EvidenceContext::default();
        context
            .add(
                &repository,
                search::search(&pool, None, query, date, 6).await.unwrap(),
                date,
            )
            .await
            .unwrap();
        assert!(
            context
                .sources
                .iter()
                .any(|s| s.anchor == anchor && s.citation.contains(law)),
            "{query}"
        );
        assert!(context.sources.len() <= MAX_SOURCES);
    }

    // Use the actual footnote pointing from ZoP to § 7 SP.
    let note = search::get_provision(&pool, "poznamky.poznamka-8", date, Some("SK/ZZ/1990/372"))
        .await
        .unwrap();
    let mut context = EvidenceContext::default();
    context.add(&repository, note, date).await.unwrap();
    assert!(
        context
            .sources
            .iter()
            .any(|s| s.anchor == "paragraf-7" && s.citation.contains("71/1967"))
    );

    // A historical date must never import a newer target version.
    let historical = NaiveDate::from_ymd_opt(2018, 9, 1).unwrap();
    let node = search::search(&pool, None, "§ 54 SP", historical, 6)
        .await
        .unwrap()
        .remove(0);
    let links = serde_json::json!([
        {"href":"../../../../SK/ZZ/2005/300/#paragraf-216","label":"§ 216 TZ"},
        {"href":"#paragraf-54.odsek-999","label":"§ 54 ods. 999"},
        {"href":"../../../../SK/ZZ/1964/40/#paragraf-1","label":"§ 1 OZ"}
    ]);
    sqlx::query("UPDATE legal_nodes SET reference_links=$2 WHERE id=$1")
        .bind(node.id)
        .bind(links)
        .execute(&pool)
        .await
        .unwrap();
    let mut context = EvidenceContext::default();
    context
        .add(&repository, vec![node], historical)
        .await
        .unwrap();
    assert!(
        context
            .sources
            .iter()
            .all(|s| s.citation.contains("71/1967"))
    );
    assert_eq!(
        context
            .issues
            .iter()
            .filter(|i| i.reason == "unavailable_at_selected_date")
            .count(),
        3
    );

    // A cyclic graph is bounded; an explicit later request can resolve a
    // previously reported depth gap, which must then disappear.
    for (from, to) in [(54, 55), (55, 56), (56, 57), (57, 54)] {
        let links =
            serde_json::json!([{"href":format!("#paragraf-{to}"),"label":format!("§ {to}")}]);
        sqlx::query("UPDATE legal_nodes SET reference_links=$2 WHERE anchor=$1 AND version_id=(SELECT id FROM act_versions WHERE act_id='SK/ZZ/1967/71')")
                .bind(format!("paragraf-{from}")).bind(links).execute(&pool).await.unwrap();
    }
    let mut context = EvidenceContext::default();
    context
        .add(
            &repository,
            search::search(&pool, None, "§ 54 SP", date, 6)
                .await
                .unwrap(),
            date,
        )
        .await
        .unwrap();
    assert_eq!(context.sources.len(), 3);
    assert!(
        context
            .issues
            .iter()
            .any(|i| i.target.ends_with("#paragraf-57") && i.reason == "reference_depth_limit")
    );
    context
        .add(
            &repository,
            search::search(&pool, None, "§ 57 SP", date, 6)
                .await
                .unwrap(),
            date,
        )
        .await
        .unwrap();
    assert_eq!(context.sources.len(), 4);
    assert!(context.issues.is_empty());

    // Restart/backfill repairs only derived links in existing snapshots.
    let (html, manifest) = crate::adapters::spi::db::import::BUNDLED_CORPORA[3];
    let manifest = serde_json::from_str(manifest).unwrap();
    crate::adapters::spi::db::import::import(&pool, html, &manifest)
        .await
        .unwrap();
    let id = search::search(&pool, None, "§ 54 SP", date, 6)
        .await
        .unwrap()[0]
        .id;
    assert!(search::reference_links(&pool, id).await.unwrap().is_empty());
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM legal_nodes")
        .fetch_one(&pool)
        .await
        .unwrap();
    crate::adapters::spi::db::import::import(&pool, html, &manifest)
        .await
        .unwrap();
    assert_eq!(
        before,
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM legal_nodes")
            .fetch_one(&pool)
            .await
            .unwrap()
    );
}
