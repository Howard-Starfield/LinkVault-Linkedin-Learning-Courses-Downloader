//! Syllabus extraction.
//!
//! The on-demand platform serves the course layout as a single V2 JSON
//! document at
//! `https://api.coursera.org/api/onDemandCourseMaterials.v2/?q=slug&slug=...`.
//! This module fetches that document, parses it into the `ModulesV1`
//! tree, and exposes a small list-courses helper for Phase 12's
//! optional "what am I enrolled in" UI.
//!
//! Isolation note: this file is owned by the `coursera/` module. It
//! uses the `coursera::client` HTTP helpers, never the LinkedIn-side
//! `live_clients`.
//!
//! Phase 4: types + parse + a stubbed `fetch_syllabus` (uses the existing
//! `client::get_json` for live fetches and a `from_value` constructor
//! for fixture-driven tests). Phase 5 wires the per-item extractors.

// Phase 4: every public symbol is consumed by later phases but not by
// the lib build yet. The blanket allow matches `config.rs`.
#![allow(dead_code)]

use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;

use crate::coursera::client;
use crate::coursera::define::{format_url, OPENCOURSE_ONDEMAND_COURSE_MATERIALS_V2};
use crate::coursera::error::{CourseraError, CourseraResult};

// ---------------------------------------------------------------------------
// Tree
// ---------------------------------------------------------------------------

/// A parsed on-demand course syllabus. `modules` is in user-facing order
/// (the orchestrator may reverse it later via `CourseraOptions::reverse`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModulesV1 {
    pub modules: Vec<ModuleV1>,
}

/// One module (a "week" in Coursera parlance).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleV1 {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub lessons: Vec<LessonV1>,
}

/// One lesson (a "module" in Coursera's UI, confusingly).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LessonV1 {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub items: Vec<ItemV2>,
}

/// One learning item (lecture / supplement / quiz / programming / etc.).
///
/// `type_name` is the discriminator — see `extractors::dispatch`.
/// `asset_id` and `raw` are kept so the extractors don't need to
/// re-fetch the syllabus.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemV2 {
    pub id: String,
    pub type_name: String,
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub asset_id: Option<String>,
    #[serde(default)]
    pub raw: Value,
}

// Pull in `serde::Serialize` so `Serialize` derives resolve without a
// separate import in the callers' files.
use serde::Serialize;

// ---------------------------------------------------------------------------
// Fetch
// ---------------------------------------------------------------------------

/// Fetch the raw V2 syllabus JSON for `slug`. The CAUTH cookie is
/// expected to be in the client's cookie jar (see `auth::AuthSession`).
///
/// Network-bound; not exercised by the default test run.
pub async fn fetch_syllabus(client: &Client, slug: &str) -> CourseraResult<Value> {
    let url = format_url(OPENCOURSE_ONDEMAND_COURSE_MATERIALS_V2, &[("slug", slug)]);
    let value: Value = client::get_json(client, &url).await?;
    Ok(value)
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// Parse a V2 syllabus JSON value into a `ModulesV1` tree. Pure: no
/// network, no filesystem.
pub fn parse_syllabus(json: &Value) -> CourseraResult<ModulesV1> {
    // Top-level fields we care about: `linked` (the element tables) and
    // `elements` (the top-level ID list — usually `moduleIds`).
    let linked = json
        .get("linked")
        .ok_or_else(|| CourseraError::SyllabusParse("missing 'linked'".to_string()))?;
    let elements = json
        .get("elements")
        .and_then(|e| e.as_array())
        .ok_or_else(|| CourseraError::SyllabusParse("missing 'elements'".to_string()))?;
    if elements.is_empty() {
        return Err(CourseraError::SyllabusParse(
            "'elements' is empty".to_string(),
        ));
    }

    // The V2 endpoint returns a single top-level element that references
    // the rest by ID in `linked`. Find it.
    let root = elements
        .first()
        .ok_or_else(|| CourseraError::SyllabusParse("empty elements[0]".to_string()))?;
    let module_ids = root
        .get("moduleIds")
        .and_then(|v| v.as_array())
        .ok_or_else(|| CourseraError::SyllabusParse("missing moduleIds".to_string()))?;

    let module_table = lookup_table(linked, "onDemandCourseMaterialModules.v1")?;
    let lesson_table = lookup_table(linked, "onDemandCourseMaterialLessons.v1")?;
    let item_table = lookup_table(linked, "onDemandCourseMaterialItems.v2")?;

    let mut modules = Vec::with_capacity(module_ids.len());
    for module_id_value in module_ids {
        let module_id = module_id_value
            .as_str()
            .ok_or_else(|| CourseraError::SyllabusParse("moduleId not a string".to_string()))?;
        let module_obj = module_table.get(module_id).ok_or_else(|| {
            CourseraError::SyllabusParse(format!("module {} not in linked", module_id))
        })?;
        let module = parse_module(module_obj, &lesson_table, &item_table)?;
        modules.push(module);
    }

    Ok(ModulesV1 { modules })
}

fn parse_module(
    module_obj: &Value,
    lesson_table: &Value,
    item_table: &Value,
) -> CourseraResult<ModuleV1> {
    let id = field_str(module_obj, "id")?;
    let slug = field_str(module_obj, "slug")?;
    let name = field_str(module_obj, "name")?;
    let lesson_ids = module_obj
        .get("lessonIds")
        .and_then(|v| v.as_array())
        .ok_or_else(|| CourseraError::SyllabusParse(format!("module {} missing lessonIds", id)))?;
    let mut lessons = Vec::with_capacity(lesson_ids.len());
    for lesson_id_value in lesson_ids {
        let lesson_id = lesson_id_value.as_str().ok_or_else(|| {
            CourseraError::SyllabusParse(format!("module {} lessonId not a string", id))
        })?;
        let lesson_obj = lesson_table.get(lesson_id).ok_or_else(|| {
            CourseraError::SyllabusParse(format!("lesson {} not in linked", lesson_id))
        })?;
        let lesson = parse_lesson(lesson_obj, item_table)?;
        lessons.push(lesson);
    }
    Ok(ModuleV1 {
        id,
        slug,
        name,
        lessons,
    })
}

fn parse_lesson(lesson_obj: &Value, item_table: &Value) -> CourseraResult<LessonV1> {
    let id = field_str(lesson_obj, "id")?;
    let slug = field_str(lesson_obj, "slug")?;
    let name = field_str(lesson_obj, "name")?;
    let element_ids = lesson_obj
        .get("elementIds")
        .and_then(|v| v.as_array())
        .ok_or_else(|| CourseraError::SyllabusParse(format!("lesson {} missing elementIds", id)))?;
    let mut items = Vec::with_capacity(element_ids.len());
    for element_id_value in element_ids {
        let element_id = element_id_value.as_str().ok_or_else(|| {
            CourseraError::SyllabusParse(format!("lesson {} elementId not a string", id))
        })?;
        let item_obj = item_table.get(element_id).ok_or_else(|| {
            CourseraError::SyllabusParse(format!("item {} not in linked", element_id))
        })?;
        items.push(parse_item(item_obj)?);
    }
    Ok(LessonV1 {
        id,
        slug,
        name,
        items,
    })
}

fn parse_item(item_obj: &Value) -> CourseraResult<ItemV2> {
    let id = field_str(item_obj, "id")?;
    let type_name = field_str(item_obj, "typeName")?;
    let name = item_obj
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let slug = item_obj
        .get("slug")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // `contentSummary.content.definition.assetId` for supplements; for
    // lectures the asset id is on a different path. Keep the raw blob
    // and let the extractors dig.
    let asset_id = extract_asset_id(item_obj);
    Ok(ItemV2 {
        id,
        type_name,
        name,
        slug,
        asset_id,
        raw: item_obj.clone(),
    })
}

fn extract_asset_id(item_obj: &Value) -> Option<String> {
    // Try the supplement / asset shape first.
    if let Some(s) = item_obj
        .pointer("/contentSummary/content/definition/assetId")
        .and_then(|v| v.as_str())
    {
        return Some(s.to_string());
    }
    // Lecture video id (the orchestrator's lecture extractor uses this).
    if let Some(s) = item_obj
        .pointer("/contentSummary/content/definition/videoId")
        .and_then(|v| v.as_str())
    {
        return Some(s.to_string());
    }
    None
}

fn field_str(obj: &Value, key: &str) -> CourseraResult<String> {
    obj.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| CourseraError::SyllabusParse(format!("missing '{}'", key)))
}

fn lookup_table<'a>(linked: &'a Value, name: &str) -> CourseraResult<&'a Value> {
    let table = linked
        .get(name)
        .ok_or_else(|| CourseraError::SyllabusParse(format!("missing linked table '{}'", name)))?;
    Ok(table)
}

// ---------------------------------------------------------------------------
// Optional list-courses (Phase 12 surface)
// ---------------------------------------------------------------------------

/// List the slugs the user is currently enrolled in. Returns the input
/// as-is for v1 — actual expansion is a follow-up. The function is
/// present so Phase 12's UI can call a real symbol.
pub async fn list_courses(_client: &Client) -> CourseraResult<Vec<String>> {
    // Punted: this requires the `OPENCOURSE_MEMBERSHIPS` endpoint and a
    // `user_id` that the orchestrator does not currently discover.
    // Returning an empty vec keeps the UI happy without making a
    // network call we can't honour.
    Ok(Vec::new())
}

/// Expand a list of specialization slugs into their member course slugs.
/// Returns the input unchanged in v1 — the specializations endpoint is
/// not yet wired.
pub async fn expand_specializations(
    _client: &Client,
    slugs: Vec<String>,
) -> CourseraResult<Vec<String>> {
    Ok(slugs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture_syllabus() -> Value {
        json!({
            "elements": [
                {
                    "id": "ROOT",
                    "slug": "ml-005",
                    "name": "Machine Learning",
                    "moduleIds": ["m1", "m2"]
                }
            ],
            "linked": {
                "onDemandCourseMaterialModules.v1": {
                    "m1": {
                        "id": "m1",
                        "slug": "intro",
                        "name": "Introduction",
                        "lessonIds": ["l1"]
                    },
                    "m2": {
                        "id": "m2",
                        "slug": "regression",
                        "name": "Regression",
                        "lessonIds": ["l2", "l3"]
                    }
                },
                "onDemandCourseMaterialLessons.v1": {
                    "l1": {
                        "id": "l1",
                        "slug": "welcome",
                        "name": "Welcome",
                        "elementIds": ["i1"]
                    },
                    "l2": {
                        "id": "l2",
                        "slug": "model-rep",
                        "name": "Model Representation",
                        "elementIds": ["i2", "i3"]
                    },
                    "l3": {
                        "id": "l3",
                        "slug": "cost-function",
                        "name": "Cost Function",
                        "elementIds": ["i4"]
                    }
                },
                "onDemandCourseMaterialItems.v2": {
                    "i1": {
                        "id": "i1",
                        "typeName": "lecture",
                        "name": "Welcome to ML",
                        "slug": "welcome-to-ml",
                        "contentSummary": {
                            "content": {
                                "typeName": "lecture",
                                "definition": {
                                    "videoId": "abc123"
                                }
                            }
                        }
                    },
                    "i2": {
                        "id": "i2",
                        "typeName": "lecture",
                        "name": "Model Representation",
                        "slug": "model-rep",
                        "contentSummary": {
                            "content": {
                                "typeName": "lecture",
                                "definition": {
                                    "videoId": "def456"
                                }
                            }
                        }
                    },
                    "i3": {
                        "id": "i3",
                        "typeName": "supplement",
                        "name": "Lecture slides",
                        "slug": "lecture-slides",
                        "contentSummary": {
                            "content": {
                                "typeName": "asset",
                                "definition": {
                                    "assetId": "asset789"
                                }
                            }
                        }
                    },
                    "i4": {
                        "id": "i4",
                        "typeName": "quiz",
                        "name": "Quiz: Regression",
                        "slug": "quiz-regression"
                    }
                }
            }
        })
    }

    #[test]
    fn parse_syllabus_walks_the_three_level_tree() {
        let json = fixture_syllabus();
        let modules = parse_syllabus(&json).unwrap();
        assert_eq!(modules.modules.len(), 2);
        assert_eq!(modules.modules[0].name, "Introduction");
        assert_eq!(modules.modules[0].lessons.len(), 1);
        assert_eq!(modules.modules[0].lessons[0].items.len(), 1);
        assert_eq!(modules.modules[0].lessons[0].items[0].type_name, "lecture");
        assert_eq!(
            modules.modules[1].lessons[0].items[1].type_name,
            "supplement"
        );
    }

    #[test]
    fn parse_syllabus_extracts_video_id_for_lectures() {
        let json = fixture_syllabus();
        let modules = parse_syllabus(&json).unwrap();
        let item = &modules.modules[0].lessons[0].items[0];
        assert_eq!(item.asset_id.as_deref(), Some("abc123"));
    }

    #[test]
    fn parse_syllabus_extracts_asset_id_for_supplements() {
        let json = fixture_syllabus();
        let modules = parse_syllabus(&json).unwrap();
        let item = &modules.modules[1].lessons[0].items[1];
        assert_eq!(item.asset_id.as_deref(), Some("asset789"));
    }

    #[test]
    fn parse_syllabus_errors_on_missing_linked() {
        let bad = json!({"elements": [{}]});
        let err = parse_syllabus(&bad).unwrap_err();
        assert!(matches!(err, CourseraError::SyllabusParse(_)));
    }

    #[test]
    fn parse_syllabus_errors_on_empty_elements() {
        let bad = json!({"elements": [], "linked": {}});
        let err = parse_syllabus(&bad).unwrap_err();
        assert!(matches!(err, CourseraError::SyllabusParse(_)));
    }

    #[test]
    fn parse_syllabus_errors_on_missing_module_in_linked() {
        let bad = json!({
            "elements": [{"moduleIds": ["missing"]}],
            "linked": {
                "onDemandCourseMaterialModules.v1": {},
                "onDemandCourseMaterialLessons.v1": {},
                "onDemandCourseMaterialItems.v2": {}
            }
        });
        let err = parse_syllabus(&bad).unwrap_err();
        assert!(matches!(err, CourseraError::SyllabusParse(_)));
    }

    #[test]
    fn parse_syllabus_errors_on_missing_lesson_table() {
        let bad = json!({
            "elements": [{"moduleIds": ["m1"]}],
            "linked": {
                "onDemandCourseMaterialModules.v1": {
                    "m1": {"id": "m1", "slug": "x", "name": "X", "lessonIds": ["l1"]}
                }
            }
        });
        let err = parse_syllabus(&bad).unwrap_err();
        assert!(matches!(err, CourseraError::SyllabusParse(_)));
    }

    #[test]
    fn parse_syllabus_handles_empty_modules_list() {
        let bad = json!({
            "elements": [{"moduleIds": []}],
            "linked": {
                "onDemandCourseMaterialModules.v1": {},
                "onDemandCourseMaterialLessons.v1": {},
                "onDemandCourseMaterialItems.v2": {}
            }
        });
        let modules = parse_syllabus(&bad).unwrap();
        assert!(modules.modules.is_empty());
    }

    #[test]
    fn format_url_for_syllabus_url_includes_slug() {
        let url = format_url(
            OPENCOURSE_ONDEMAND_COURSE_MATERIALS_V2,
            &[("slug", "ml-005")],
        );
        assert!(url.contains("slug=ml-005"));
        assert!(url.contains("onDemandCourseMaterials.v2"));
    }

    #[test]
    fn expand_specializations_returns_input_unchanged_in_v1() {
        // This is a synchronous smoke test of the identity behaviour
        // documented in the doc comment.
        let input = vec!["a".to_string(), "b".to_string()];
        let rt = tokio::runtime::Runtime::new().unwrap();
        let out = rt
            .block_on(expand_specializations(&dummy_client(), input.clone()))
            .unwrap();
        assert_eq!(out, input);
    }

    #[test]
    fn list_courses_returns_empty_in_v1() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let out = rt.block_on(list_courses(&dummy_client())).unwrap();
        assert!(out.is_empty());
    }

    fn dummy_client() -> Client {
        crate::coursera::client::build_client().unwrap()
    }
}
