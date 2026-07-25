use super::models::{EditionKind, NewspaperEdition, PublicationSchedule};
use chrono::NaiveDate;
use regex::Regex;
use std::collections::HashSet;

const BASE_URL: &str = "https://ep.worldjournal.com";

struct BuiltInEdition {
    code: &'static str,
    name_zh: &'static str,
    name_en: &'static str,
    kind: EditionKind,
    schedule: PublicationSchedule,
}

const BUILT_INS: [BuiltInEdition; 13] = [
    BuiltInEdition {
        code: "NY",
        name_zh: "紐約",
        name_en: "New York",
        kind: EditionKind::Daily,
        schedule: PublicationSchedule::Daily,
    },
    BuiltInEdition {
        code: "LA",
        name_zh: "洛杉磯",
        name_en: "Los Angeles",
        kind: EditionKind::Daily,
        schedule: PublicationSchedule::Daily,
    },
    BuiltInEdition {
        code: "SF",
        name_zh: "舊金山",
        name_en: "San Francisco",
        kind: EditionKind::Daily,
        schedule: PublicationSchedule::Daily,
    },
    BuiltInEdition {
        code: "NJ",
        name_zh: "新賓",
        name_en: "New Jersey / Pennsylvania",
        kind: EditionKind::Daily,
        schedule: PublicationSchedule::Daily,
    },
    BuiltInEdition {
        code: "DC",
        name_zh: "大華府",
        name_en: "Washington, D.C.",
        kind: EditionKind::Daily,
        schedule: PublicationSchedule::Daily,
    },
    BuiltInEdition {
        code: "BO",
        name_zh: "波士頓",
        name_en: "Boston",
        kind: EditionKind::Daily,
        schedule: PublicationSchedule::Daily,
    },
    BuiltInEdition {
        code: "AT",
        name_zh: "美東南",
        name_en: "Southeast U.S.",
        kind: EditionKind::Daily,
        schedule: PublicationSchedule::Daily,
    },
    BuiltInEdition {
        code: "CH",
        name_zh: "芝加哥",
        name_en: "Chicago",
        kind: EditionKind::Daily,
        schedule: PublicationSchedule::Daily,
    },
    BuiltInEdition {
        code: "TX",
        name_zh: "德州",
        name_en: "Texas",
        kind: EditionKind::Daily,
        schedule: PublicationSchedule::Daily,
    },
    BuiltInEdition {
        code: "SE",
        name_zh: "西雅圖／夏威夷",
        name_en: "Seattle / Hawaii",
        kind: EditionKind::Daily,
        schedule: PublicationSchedule::Daily,
    },
    BuiltInEdition {
        code: "NW",
        name_zh: "世界周刊（美東）",
        name_en: "World Journal Weekly — East",
        kind: EditionKind::Weekly,
        schedule: PublicationSchedule::WeeklySunday,
    },
    BuiltInEdition {
        code: "LW",
        name_zh: "世界周刊（美西南）",
        name_en: "World Journal Weekly — Southwest",
        kind: EditionKind::Weekly,
        schedule: PublicationSchedule::WeeklySunday,
    },
    BuiltInEdition {
        code: "SW",
        name_zh: "世界周刊（美西北）",
        name_en: "World Journal Weekly — Northwest",
        kind: EditionKind::Weekly,
        schedule: PublicationSchedule::WeeklySunday,
    },
];

pub fn built_in_catalog() -> Vec<NewspaperEdition> {
    BUILT_INS
        .iter()
        .map(|edition| NewspaperEdition {
            code: edition.code.to_string(),
            name_zh: edition.name_zh.to_string(),
            name_en: edition.name_en.to_string(),
            kind: edition.kind,
            schedule: edition.schedule,
            source_url: format!("{BASE_URL}/{}", edition.code),
            publication_date: None,
            discovered: false,
        })
        .collect()
}

pub fn special_edition(
    code: &str,
    name_zh: &str,
    publication_date: NaiveDate,
) -> Option<NewspaperEdition> {
    if code.len() != 2 || !code.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return None;
    }
    if name_zh.trim().is_empty() {
        return None;
    }

    Some(NewspaperEdition {
        code: code.to_string(),
        name_zh: name_zh.trim().to_string(),
        name_en: "Special publication".to_string(),
        kind: EditionKind::Special,
        schedule: PublicationSchedule::AdHoc,
        source_url: format!("{BASE_URL}/{code}/{publication_date}"),
        publication_date: Some(publication_date),
        discovered: true,
    })
}

pub fn merge_catalog(
    built_ins: Vec<NewspaperEdition>,
    discovered: Vec<NewspaperEdition>,
) -> Vec<NewspaperEdition> {
    let mut seen = HashSet::new();
    let mut merged = Vec::new();

    for edition in built_ins.into_iter().chain(discovered) {
        let key = (
            edition.code.clone(),
            edition.publication_date,
            edition.name_zh.clone(),
        );
        if seen.insert(key) {
            merged.push(edition);
        }
    }
    merged
}

pub fn discover_specials(homepage_html: &str) -> Vec<NewspaperEdition> {
    let tag_pattern = Regex::new(r#"<a\b[^>]*>"#).expect("static tag regex must compile");
    let href_pattern = Regex::new(r#"href=["']/([A-Z]{2})/(\d{4}-\d{2}-\d{2})["']"#)
        .expect("static href regex must compile");
    let label_pattern =
        Regex::new(r#"aria-label=["']([^"']+)["']"#).expect("static label regex must compile");
    let mut discovered = Vec::new();

    for tag in tag_pattern.find_iter(homepage_html) {
        let Some(href) = href_pattern.captures(tag.as_str()) else {
            continue;
        };
        let code = &href[1];
        if !matches!(code, "EA" | "ED") {
            continue;
        }
        let Some(label) = label_pattern.captures(tag.as_str()) else {
            continue;
        };
        let Ok(date) = NaiveDate::parse_from_str(&href[2], "%Y-%m-%d") else {
            continue;
        };
        let title = label[1].trim().trim_end_matches("選單").trim();
        if let Some(edition) = special_edition(code, title, date) {
            discovered.push(edition);
        }
    }

    merge_catalog(Vec::new(), discovered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_catalog_contains_ten_daily_and_three_weekly_editions() {
        let catalog = built_in_catalog();
        assert_eq!(catalog.len(), 13);
        assert_eq!(
            catalog
                .iter()
                .filter(|item| item.kind == EditionKind::Daily)
                .count(),
            10
        );
        assert_eq!(
            catalog
                .iter()
                .filter(|item| item.kind == EditionKind::Weekly)
                .count(),
            3
        );
        assert!(!catalog.iter().any(|item| item.code == "EA"));
    }

    #[test]
    fn dated_specials_with_the_same_code_remain_distinct() {
        let first = special_edition(
            "EA",
            "馬年春節專刊",
            NaiveDate::from_ymd_opt(2026, 2, 17).unwrap(),
        )
        .unwrap();
        let second = special_edition(
            "EA",
            "2026報稅新攻略",
            NaiveDate::from_ymd_opt(2026, 3, 1).unwrap(),
        )
        .unwrap();

        let merged = merge_catalog(built_in_catalog(), vec![first, second]);
        assert_eq!(merged.iter().filter(|item| item.code == "EA").count(), 2);
    }

    #[test]
    fn homepage_discovery_extracts_dated_specials_and_ignores_regular_editions() {
        let html = r#"
            <a href="/NY/2026-07-24" aria-label="紐約選單">紐約</a>
            <a href="/EA/2026-03-01" aria-label="2026報稅新攻略選單">Special</a>
            <a href="/EA/2026-03-01" aria-label="2026報稅新攻略選單">Duplicate</a>
            <a href="/ED/2025-04-05" aria-label="美西教育專刊(春季版)選單">Special</a>
        "#;

        let discovered = discover_specials(html);
        assert_eq!(discovered.len(), 2);
        assert_eq!(discovered[0].name_zh, "2026報稅新攻略");
        assert_eq!(discovered[1].code, "ED");
    }
}
