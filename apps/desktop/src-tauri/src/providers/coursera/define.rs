//! URL templates and constants ported from `coursera-dl/coursera/define.py`.
//!
//! These are the **only** Coursera endpoints the rest of the module talks to.
//! They are kept as `&'static str` constants so no allocation is needed for
//! URL formatting; the orchestrator uses [`format_url`] to substitute
//! `{placeholders}` at call time.
//!
//! Isolation note: this file is owned by the `coursera/` module. The
//! LinkedIn side has its own endpoints and does not import anything from
//! here.

// Phase 3: every public symbol is consumed by later phases but not by
// the lib build yet. The blanket allow matches `config.rs` and is
// removed as each symbol gets its first non-test caller.
#![allow(dead_code)]

/// HTTP 403 — "Forbidden" — used by the syllabus parser to detect a bad
/// CAUTH (Coursera returns 403 for unauthenticated syllabus fetches).
pub const HTTP_FORBIDDEN: u16 = 403;

/// Base URL for `api.coursera.org`.
pub const COURSERA_URL: &str = "https://api.coursera.org";

/// V1 login endpoint (kept for parity, not used by the active login flow).
pub const AUTH_URL: &str = "https://accounts.coursera.org/api/v1/login";

/// V3 login endpoint. POST email + password, get `CAUTH` from `Set-Cookie`.
pub const AUTH_URL_V3: &str = "https://api.coursera.org/api/login/v3";

/// Per-class landing page. Used to validate a CAUTH.
pub const CLASS_URL: &str = "https://class.coursera.org/{class_name}";

/// On-demand lecture videos endpoint.
pub const OPENCOURSE_ONDEMAND_LECTURE_VIDEOS_V1: &str =
    "https://api.coursera.org/api/onDemandLectureVideos.v1/{course_id}~{video_id}?\
includes=video&\
fields=onDemandVideos.v1(sources%2Csubtitles%2CsubtitlesVtt%2CsubtitlesTxt)";

/// On-demand lecture assets endpoint (PDF, PPTX, CSV attached to a lecture).
pub const OPENCOURSE_ONDEMAND_LECTURE_ASSETS_V1: &str =
    "https://api.coursera.org/api/onDemandLectureAssets.v1/{course_id}~{video_id}/?includes=openCourseAssets";

/// On-demand supplement endpoint (reading material).
pub const OPENCOURSE_ONDEMAND_SUPPLEMENT_V1: &str =
    "https://api.coursera.org/api/onDemandSupplements.v1/\
{course_id}~{element_id}?includes=asset&\
fields=openCourseAssets.v1%28typeName%29,openCourseAssets.v1%28definition%29";

/// On-demand programming assignment endpoint (graded / ungraded / phased).
pub const OPENCOURSE_ONDEMAND_PROGRAMMING_V1: &str =
    "https://api.coursera.org/api/onDemandProgrammingLearnerAssignments.v1/\
{course_id}~{element_id}?fields=submissionLearnerSchema";

/// On-demand references endpoint (the "Resources" tab).
pub const OPENCOURSE_ONDEMAND_REFERENCES_V1: &str =
    "https://api.coursera.org/api/onDemandReferences.v1/?courseId={course_id}\
&q=courseListed&fields=name%2CshortId%2Cslug%2Ccontent&includes=assets";

/// On-demand asset URL resolver. Returns the signed URL for one or more
/// asset ids.
pub const OPENCOURSE_ASSET_URL_V1: &str = "https://api.coursera.org/api/assetUrls.v1?ids={ids}";

/// On-demand course materials V2 (the syllabus endpoint we use).
pub const OPENCOURSE_ONDEMAND_COURSE_MATERIALS_V2: &str =
    "https://api.coursera.org/api/onDemandCourseMaterials.v2/?q=slug&slug={slug}\
&includes=modules%2Clessons%2CpassableItemGroups%2CpassableItemGroupChoices%2CpassableLessonElements%2Citems%2Ctracks%2CgradePolicy&\
fields=moduleIds%2ConDemandCourseMaterialModules.v1(name%2Cslug%2Cdescription%2CtimeCommitment%2ClessonIds%2Coptional%2ClearningObjectives)\
%2ConDemandCourseMaterialLessons.v1(name%2Cslug%2CtimeCommitment%2CelementIds%2Coptional%2CtrackId)\
%2ConDemandCourseMaterialPassableItemGroups.v1(requiredPassedCount%2CpassableItemGroupChoiceIds%2CtrackId)\
%2ConDemandCourseMaterialPassableItemGroupChoices.v1(name%2Cdescription%2CitemIds)\
%2ConDemandCourseMaterialPassableLessonElements.v1(gradingWeight%2CisRequiredForPassing)\
%2ConDemandCourseMaterialItems.v2(name%2Cslug%2CtimeCommitment%2CcontentSummary%2CisLocked%2ClockableByItem%2CitemLockedReasonCode%2CtrackId%2ClockedStatus%2CitemLockSummary)\
%2ConDemandCourseMaterialTracks.v1(passablesCount)\
&showLockedItems=true";

/// "About this course" metadata endpoint.
pub const ABOUT_URL: &str = "https://api.coursera.org/api/catalog.v1/courses?\
fields=largeIcon,photo,previewLink,shortDescription,smallIcon,smallIconHover,\
universityLogo,universityLogoSt,video,videoId,aboutTheCourse,targetAudience,\
faq,courseSyllabus,courseFormat,suggestedReadings,instructor,\
estimatedClassWorkload,aboutTheInstructor,recommendedBackground,\
subtitleLanguagesCsv&q=search&query={slug}";

/// On-demand course metadata (for the "About" panel).
pub const OPENCOURSE_ONDEMAND_COURSES_V1: &str =
    "https://api.coursera.org/api/onDemandCourses.v1?q=slug&slug={slug}&\
includes=instructorIds%2CpartnerIds%2C_links&\
fields=brandingImage%2CcertificatePurchaseEnabledAt%2C\
partners.v1(squareLogo%2CrectangularLogo)%2C\
instructors.v1(fullName)%2CoverridePartnerLogos%2CsessionsEnabledAt%2C\
domainTypes%2CpremiumExperienceVariant%2CisRestrictedMembership";

/// User agent sent with every request. Coursera is fine with the `LinkedVault`
/// token; no UA is required to match a particular string.
pub const USER_AGENT: &str = "LinkedVault/0.1 (+coursera; rust)";

/// Marker used by the supplement extractor to flag an in-memory HTML body
/// (rather than a remote URL) in the unified `ResourceLink::url` field.
/// The downloader strips this before deciding where to write.
pub const IN_MEMORY_MARKER: &str = "#inmemory#";

/// Extension written for in-memory supplemental HTML bodies.
pub const IN_MEMORY_EXTENSION: &str = "html";

/// MathJax CDN URL injected into quiz/exam HTML pages. Mirrors the Python
/// tool's `INSTRUCTIONS_HTML_MATHJAX_URL`.
pub const INSTRUCTIONS_HTML_MATHJAX_URL: &str = "https://cdn.mathjax.org/mathjax/latest/MathJax.js";

/// The on-demand specializations endpoint (placeholder; specialization
/// expansion is punted to a follow-up).
pub const OPENCOURSE_ONDEMAND_SPECIALIZATIONS_V1: &str =
    "https://api.coursera.org/api/onDemandSpecializations.v1?q=slug&slug={slug}";

/// Substitute `{key}` placeholders in `template` with the values in
/// `subs`. Missing keys are left as-is (matches the Python tool's
/// permissive behaviour).
///
/// This is a tiny, allocation-light replacement for pulling in a full
/// templating crate.
pub fn format_url(template: &str, subs: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        if let Some(close) = after.find('}') {
            let key = &after[..close];
            let value = subs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| *v)
                .unwrap_or("");
            out.push_str(value);
            rest = &after[close + 1..];
        } else {
            // Unterminated `{` — emit the rest verbatim.
            out.push_str(&rest[open..]);
            return out;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_url_substitutes_single_key() {
        assert_eq!(
            format_url("hello/{name}", &[("name", "world")]),
            "hello/world"
        );
    }

    #[test]
    fn format_url_substitutes_multiple_keys() {
        assert_eq!(
            format_url("https://x/{a}/{b}", &[("a", "foo"), ("b", "bar")]),
            "https://x/foo/bar"
        );
    }

    #[test]
    fn format_url_leaves_missing_key_blank() {
        // The Python tool silently substitutes '' for missing keys.
        assert_eq!(format_url("a/{x}/b", &[]), "a//b");
    }

    #[test]
    fn format_url_handles_no_placeholders() {
        assert_eq!(format_url("https://api/", &[]), "https://api/");
    }

    #[test]
    fn format_url_handles_unterminated_brace() {
        // No closing `}` — emit verbatim.
        assert_eq!(format_url("a/{b", &[("b", "x")]), "a/{b");
    }

    #[test]
    fn format_url_repeats_the_same_key() {
        // Useful for URLs that reuse the same slug twice.
        assert_eq!(
            format_url("/learn/{slug}/v/{slug}", &[("slug", "ml-005")]),
            "/learn/ml-005/v/ml-005"
        );
    }

    #[test]
    fn class_url_renders_with_a_slug() {
        let url = format_url(CLASS_URL, &[("class_name", "ml-005")]);
        assert_eq!(url, "https://class.coursera.org/ml-005");
    }

    #[test]
    fn syllabus_url_renders_with_a_slug() {
        let url = format_url(
            OPENCOURSE_ONDEMAND_COURSE_MATERIALS_V2,
            &[("slug", "machine-learning")],
        );
        assert!(url.contains("slug=machine-learning"));
        assert!(url.starts_with("https://api.coursera.org/api/onDemandCourseMaterials.v2/"));
    }

    #[test]
    fn in_memory_marker_is_stable() {
        // The marker is part of the downloader contract; lock it in.
        assert_eq!(IN_MEMORY_MARKER, "#inmemory#");
    }
}
