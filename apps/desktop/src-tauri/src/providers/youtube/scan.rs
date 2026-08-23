use crate::providers::youtube::error::YouTubeInternalError;
use crate::providers::youtube::helper::{invocation, output_error, MAX_DISCOVERY_STDOUT_BYTES};
use crate::providers::youtube::models::{
    PlannedYouTubeTranscriptInspection, ScanYouTubeSourceRequest, ScanYouTubeSourceResponse,
    YouTubeAvailability, YouTubePlaylistMode, YouTubeScanItem,
};
use crate::workflow::transient::managed_process::run;
use crate::workflow::transient::DiscoveryOperation;
use chrono::{Duration as ChronoDuration, Utc};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use url::Url;

pub const MAX_SOURCE_URL_BYTES: usize = 4096;
pub const MAX_SCAN_ENTRIES: usize = 500;
pub const PLAN_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Debug)]
pub struct PlannedYouTubeItem {
    pub public: YouTubeScanItem,
    pub source_url: String,
    pub transcript_inspection: Option<PlannedYouTubeTranscriptInspection>,
}

#[derive(Clone, Debug)]
pub struct YouTubeScanPlan {
    pub response: ScanYouTubeSourceResponse,
    pub items: Vec<PlannedYouTubeItem>,
    pub source_snapshot_digest: String,
    pub expires_at: Instant,
}

#[derive(Clone, Debug)]
pub struct ValidatedSource {
    pub kind: YouTubePlaylistMode,
    pub video_id: Option<String>,
    pub playlist_id: Option<String>,
    pub canonical_url: String,
}

pub fn scan_source(
    request: &ScanYouTubeSourceRequest,
    operation: &DiscoveryOperation,
) -> Result<YouTubeScanPlan, YouTubeInternalError> {
    let source = validate_url(&request.url, request.playlist_mode.clone())?;
    let stdout = discovery_stdout(&source, operation)?;
    parse_scan_plan(source, &stdout)
}

/// Re-runs discovery against the canonical source captured by a scan plan and
/// returns the current metadata for the selected occurrences.  Display-only
/// metadata may drift, but source identity, occurrence identity, and
/// availability remain immutable admission inputs.
pub fn revalidate_selected_source(
    plan: &YouTubeScanPlan,
    selected_occurrence_ids: &[String],
    operation: &DiscoveryOperation,
) -> Result<Vec<PlannedYouTubeItem>, YouTubeInternalError> {
    let source = validate_url(
        &plan.response.canonical_url,
        Some(plan.response.kind.clone()),
    )?;
    let stdout = discovery_stdout(&source, operation)?;
    let current = parse_scan_plan(source, &stdout)?;
    revalidate_selected_items(plan, &current, selected_occurrence_ids)
}

fn discovery_stdout(
    source: &ValidatedSource,
    operation: &DiscoveryOperation,
) -> Result<String, YouTubeInternalError> {
    let args = discovery_args(source);
    let cancel = operation.cancellation_flag();
    let output = run(
        invocation(args, MAX_DISCOVERY_STDOUT_BYTES),
        None,
        cancel.as_deref(),
    )
    .map_err(|error| YouTubeInternalError::Helper(error.to_string()))?;
    if operation.cancellation_requested() || output.cancelled {
        return Err(YouTubeInternalError::InvalidRequest(
            "discovery was cancelled".to_string(),
        ));
    }
    if output.timed_out {
        return Err(YouTubeInternalError::Helper(
            "discovery timed out".to_string(),
        ));
    }
    if output.stdout_truncated {
        return Err(YouTubeInternalError::Helper(
            "discovery output exceeded the safety limit".to_string(),
        ));
    }
    if !output.status.success() {
        return Err(YouTubeInternalError::Helper(output_error(&output)));
    }
    Ok(output.stdout)
}

pub fn validate_url(
    raw: &str,
    requested_mode: Option<YouTubePlaylistMode>,
) -> Result<ValidatedSource, YouTubeInternalError> {
    if raw.len() > MAX_SOURCE_URL_BYTES || raw.trim() != raw || raw.is_empty() {
        return Err(YouTubeInternalError::InvalidUrl(
            "source URL is empty or too long".to_string(),
        ));
    }
    let parsed =
        Url::parse(raw).map_err(|error| YouTubeInternalError::InvalidUrl(error.to_string()))?;
    if parsed.scheme() != "https" || parsed.username() != "" || parsed.password().is_some() {
        return Err(YouTubeInternalError::InvalidUrl(
            "only credential-free HTTPS YouTube URLs are supported".to_string(),
        ));
    }
    let host = parsed
        .host_str()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if !matches!(
        host.as_str(),
        "youtube.com"
            | "www.youtube.com"
            | "m.youtube.com"
            | "music.youtube.com"
            | "youtu.be"
            | "www.youtu.be"
    ) {
        return Err(YouTubeInternalError::InvalidUrl(
            "host is not an allowed YouTube host".to_string(),
        ));
    }
    let video_id = if host.ends_with("youtu.be") {
        parsed
            .path()
            .trim_matches('/')
            .split('/')
            .next()
            .filter(|id| valid_id(id))
            .map(str::to_string)
    } else if parsed.path() == "/watch" {
        parsed
            .query_pairs()
            .find(|(key, _)| key == "v")
            .and_then(|(_, value)| valid_id(&value).then(|| value.to_string()))
    } else if parsed.path().starts_with("/shorts/") || parsed.path().starts_with("/live/") {
        parsed
            .path()
            .split('/')
            .nth(2)
            .filter(|id| valid_id(id))
            .map(str::to_string)
    } else {
        None
    };
    let playlist_id = parsed
        .query_pairs()
        .find(|(key, _)| key == "list")
        .and_then(|(_, value)| valid_playlist_id(&value).then(|| value.to_string()));
    let mode = requested_mode.unwrap_or_else(|| {
        if playlist_id.is_some() && video_id.is_none() {
            YouTubePlaylistMode::Playlist
        } else {
            YouTubePlaylistMode::Video
        }
    });
    match mode {
        YouTubePlaylistMode::Video => {
            let video_id = video_id.ok_or_else(|| {
                YouTubeInternalError::InvalidUrl(
                    "video URL must contain a valid video id".to_string(),
                )
            })?;
            Ok(ValidatedSource {
                kind: YouTubePlaylistMode::Video,
                canonical_url: format!("https://www.youtube.com/watch?v={video_id}"),
                video_id: Some(video_id),
                playlist_id: None,
            })
        }
        YouTubePlaylistMode::Playlist => {
            let playlist_id = playlist_id.ok_or_else(|| {
                YouTubeInternalError::InvalidUrl(
                    "playlist URL must contain a valid list id".to_string(),
                )
            })?;
            Ok(ValidatedSource {
                kind: YouTubePlaylistMode::Playlist,
                canonical_url: format!("https://www.youtube.com/playlist?list={playlist_id}"),
                video_id: None,
                playlist_id: Some(playlist_id),
            })
        }
    }
}

fn discovery_args(source: &ValidatedSource) -> Vec<String> {
    let mut args = vec![
        "--ignore-config".to_string(),
        "--no-plugin-dirs".to_string(),
        "--no-update".to_string(),
        "--no-warnings".to_string(),
        "--dump-single-json".to_string(),
        "--skip-download".to_string(),
    ];
    if matches!(source.kind, YouTubePlaylistMode::Playlist) {
        args.push("--flat-playlist".to_string());
        args.push("--playlist-end".to_string());
        args.push((MAX_SCAN_ENTRIES + 1).to_string());
    } else {
        args.push("--no-playlist".to_string());
    }
    args.push(source.canonical_url.clone());
    args
}

fn parse_scan_plan(
    source: ValidatedSource,
    stdout: &str,
) -> Result<YouTubeScanPlan, YouTubeInternalError> {
    let value = parse_json(stdout)?;
    let (title, entries, root) = match &source.kind {
        YouTubePlaylistMode::Playlist => {
            let title = value
                .get("title")
                .and_then(value_string)
                .unwrap_or_else(|| "YouTube playlist".to_string());
            let entries = value
                .get("entries")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            (title, entries, value.clone())
        }
        YouTubePlaylistMode::Video => {
            let title = value
                .get("title")
                .and_then(value_string)
                .unwrap_or_else(|| "YouTube video".to_string());
            (title, vec![value.clone()], value.clone())
        }
    };
    let truncated = entries.len() > MAX_SCAN_ENTRIES;
    let mut items = Vec::new();
    for (index, entry) in entries.into_iter().take(MAX_SCAN_ENTRIES).enumerate() {
        let Some(video_id) = entry
            .get("id")
            .and_then(value_string)
            .filter(|id| valid_id(id))
        else {
            continue;
        };
        let ordinal = index as u32 + 1;
        let item_source_url = format!("https://www.youtube.com/watch?v={video_id}");
        let title = entry
            .get("title")
            .and_then(value_string)
            .unwrap_or_else(|| format!("YouTube video {video_id}"));
        let channel_name = entry
            .get("channel")
            .and_then(value_string)
            .or_else(|| entry.get("uploader").and_then(value_string));
        let channel_id = entry.get("channel_id").and_then(value_string);
        let channel_identity = channel_id
            .as_deref()
            .or(channel_name.as_deref())
            .unwrap_or_default();
        let duration_seconds = entry
            .get("duration")
            .and_then(|value| value.as_f64())
            .filter(|value| value.is_finite() && *value >= 0.0 && *value <= u64::MAX as f64)
            .map(|value| value as u64);
        let thumbnail_available = entry.get("thumbnail").and_then(value_string).is_some();
        let availability = availability(&entry);
        let occurrence_id = digest(&format!(
            "1|{}|{}|{}|{}",
            source.canonical_url,
            source.playlist_id.as_deref().unwrap_or("single"),
            ordinal,
            video_id
        ));
        let metadata_digest = digest(&format!(
            "1|{}|{}|{}|{}|{}|{}",
            video_id,
            item_source_url,
            title,
            channel_identity,
            duration_seconds.map_or_else(String::new, |duration| duration.to_string()),
            availability_label(&availability),
        ));
        items.push(PlannedYouTubeItem {
            public: YouTubeScanItem {
                occurrence_id,
                video_id,
                source_url: item_source_url.clone(),
                title,
                ordinal,
                channel_name,
                channel_id,
                duration_seconds,
                thumbnail_available,
                availability,
                metadata_digest,
            },
            source_url: item_source_url,
            transcript_inspection: None,
        });
    }
    if items.is_empty() {
        return Err(YouTubeInternalError::Helper(
            "yt-dlp returned no usable video entries".to_string(),
        ));
    }
    let source_snapshot_digest = snapshot_digest(&source, &items);
    let scan_plan_id = opaque_id("plan");
    let response = ScanYouTubeSourceResponse {
        scan_plan_id,
        expires_at: (Utc::now() + ChronoDuration::seconds(PLAN_TTL.as_secs() as i64)).to_rfc3339(),
        kind: source.kind.clone(),
        title,
        source_id: source
            .playlist_id
            .clone()
            .or_else(|| source.video_id.clone())
            .unwrap_or_else(|| value.get("id").and_then(value_string).unwrap_or_default()),
        canonical_url: source.canonical_url,
        playlist_id: source.playlist_id,
        truncated,
        items: items.iter().map(|item| item.public.clone()).collect(),
    };
    let _ = root;
    Ok(YouTubeScanPlan {
        response,
        items,
        source_snapshot_digest,
        expires_at: Instant::now() + PLAN_TTL,
    })
}

fn snapshot_digest(source: &ValidatedSource, items: &[PlannedYouTubeItem]) -> String {
    let mut hasher = Sha256::new();
    hasher.update([1]);
    update_digest_part(&mut hasher, &source.canonical_url);
    update_digest_part(
        &mut hasher,
        source.playlist_id.as_deref().unwrap_or("single"),
    );
    for item in items {
        update_digest_part(&mut hasher, &item.public.occurrence_id);
        update_digest_part(&mut hasher, &item.public.video_id);
        update_digest_part(&mut hasher, &item.public.ordinal.to_string());
    }
    format!("{:x}", hasher.finalize())
}

fn update_digest_part(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
}

fn parse_json(stdout: &str) -> Result<serde_json::Value, YouTubeInternalError> {
    if stdout.len() > MAX_DISCOVERY_STDOUT_BYTES {
        return Err(YouTubeInternalError::Helper(
            "discovery output exceeded the safety limit".to_string(),
        ));
    }
    serde_json::from_str(stdout).map_err(|_| {
        YouTubeInternalError::Helper(
            "yt-dlp returned malformed machine-readable output".to_string(),
        )
    })
}

fn value_string(value: &serde_json::Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

fn availability(value: &serde_json::Value) -> YouTubeAvailability {
    let status = value
        .get("availability")
        .and_then(value_string)
        .map(|status| status.trim().to_ascii_lowercase());
    match status.as_deref() {
        Some("private")
        | Some("restricted")
        | Some("unavailable")
        | Some("needs_auth")
        | Some("premium_only")
        | Some("subscriber_only") => YouTubeAvailability::Unavailable,
        Some("unknown") => YouTubeAvailability::Unknown,
        Some("public") => YouTubeAvailability::Available,
        Some(_) => YouTubeAvailability::Unknown,
        None => YouTubeAvailability::Unknown,
    }
}

fn availability_label(value: &YouTubeAvailability) -> &'static str {
    match value {
        YouTubeAvailability::Available => "available",
        YouTubeAvailability::Unavailable => "unavailable",
        YouTubeAvailability::Unknown => "unknown",
    }
}

fn revalidate_selected_items(
    plan: &YouTubeScanPlan,
    current: &YouTubeScanPlan,
    selected_occurrence_ids: &[String],
) -> Result<Vec<PlannedYouTubeItem>, YouTubeInternalError> {
    if selected_occurrence_ids.is_empty() {
        return Err(YouTubeInternalError::EmptySelection);
    }
    if plan.response.kind != current.response.kind
        || plan.response.source_id != current.response.source_id
        || plan.response.canonical_url != current.response.canonical_url
        || plan.response.playlist_id != current.response.playlist_id
    {
        return Err(YouTubeInternalError::ScanPlanStale);
    }

    let mut seen = std::collections::HashSet::with_capacity(selected_occurrence_ids.len());
    let mut selected = Vec::with_capacity(selected_occurrence_ids.len());
    for occurrence_id in selected_occurrence_ids {
        if !seen.insert(occurrence_id) {
            return Err(YouTubeInternalError::DuplicateOccurrence);
        }
        let previous = plan
            .items
            .iter()
            .find(|item| item.public.occurrence_id == occurrence_id.as_str())
            .ok_or(YouTubeInternalError::UnknownOccurrence)?;
        let current_item = current
            .items
            .iter()
            .find(|item| item.public.ordinal == previous.public.ordinal)
            .ok_or(YouTubeInternalError::ScanPlanStale)?;
        if current_item.public.occurrence_id != previous.public.occurrence_id
            || current_item.public.video_id != previous.public.video_id
            || current_item.public.availability != previous.public.availability
            || matches!(
                &current_item.public.availability,
                &YouTubeAvailability::Unavailable
            )
        {
            return Err(YouTubeInternalError::ScanPlanStale);
        }
        selected.push(current_item.clone());
    }
    Ok(selected)
}

fn valid_id(value: &str) -> bool {
    (6..=32).contains(&value.len())
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn valid_playlist_id(value: &str) -> bool {
    (4..=128).contains(&value.len())
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn digest(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn opaque_id(prefix: &str) -> String {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    digest(&format!("{prefix}|{}|{now}|{sequence}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_supported_url_shapes_and_rejects_hosts() {
        let source = validate_url("https://www.youtube.com/watch?v=abc123_XY", None).unwrap();
        assert_eq!(source.video_id.as_deref(), Some("abc123_XY"));
        let playlist =
            validate_url("https://www.youtube.com/playlist?list=PL_12345", None).unwrap();
        assert!(matches!(playlist.kind, YouTubePlaylistMode::Playlist));
        assert!(validate_url("https://example.com/watch?v=abc123_XY", None).is_err());
        assert!(validate_url("http://www.youtube.com/watch?v=abc123_XY", None).is_err());
    }

    #[test]
    fn duplicate_playlist_occurrences_have_distinct_ids() {
        let source = validate_url("https://www.youtube.com/playlist?list=PL_12345", None).unwrap();
        let payload = r#"{"id":"PL_12345","title":"Playlist","entries":[{"id":"abc123_XY","title":"A"},{"id":"abc123_XY","title":"A again"}]}"#;
        let plan = parse_scan_plan(source, payload).unwrap();
        assert_eq!(plan.items.len(), 2);
        assert_ne!(
            plan.items[0].public.occurrence_id,
            plan.items[1].public.occurrence_id
        );
        assert_eq!(plan.items[0].public.ordinal, 1);
        assert_eq!(plan.items[1].public.ordinal, 2);
        assert_eq!(plan.source_snapshot_digest.len(), 64);
        let same_source =
            validate_url("https://www.youtube.com/playlist?list=PL_12345", None).unwrap();
        let repeated = parse_scan_plan(same_source, payload).unwrap();
        assert_eq!(plan.source_snapshot_digest, repeated.source_snapshot_digest);

        let reversed_source =
            validate_url("https://www.youtube.com/playlist?list=PL_12345", None).unwrap();
        let reversed = parse_scan_plan(
            reversed_source,
            r#"{"id":"PL_12345","title":"Playlist","entries":[{"id":"def456_ZZ","title":"B"},{"id":"abc123_XY","title":"A"}]}"#,
        )
        .unwrap();
        assert_ne!(plan.source_snapshot_digest, reversed.source_snapshot_digest);
    }

    #[test]
    fn oversized_or_invalid_machine_output_is_rejected() {
        assert!(parse_json("not-json").is_err());
        assert!(parse_json(&"x".repeat(MAX_DISCOVERY_STDOUT_BYTES + 1)).is_err());
        assert!(parse_json("{\"id\":\"abc123_XY\"}\ntrailing garbage").is_err());
        assert!(parse_json("{\"id\":\"abc123_XY\"}\n{\"id\":\"def456_ZZ\"}").is_err());
    }

    #[test]
    fn playlist_discovery_is_bounded_one_past_the_visible_limit() {
        let source = validate_url("https://www.youtube.com/playlist?list=PL_12345", None).unwrap();
        let args = discovery_args(&source);
        let index = args
            .iter()
            .position(|argument| argument == "--playlist-end")
            .unwrap();
        assert_eq!(args[index + 1], (MAX_SCAN_ENTRIES + 1).to_string());
    }

    #[test]
    fn availability_fixture_is_fail_closed_and_live_cannot_override_restrictions() {
        let fixtures = [
            (
                r#"{"availability":"public"}"#,
                YouTubeAvailability::Available,
            ),
            (
                r#"{"availability":"public","is_live":true}"#,
                YouTubeAvailability::Available,
            ),
            (r#"{"is_live":true}"#, YouTubeAvailability::Unknown),
            (
                r#"{"availability":" PRIVATE ","is_live":true}"#,
                YouTubeAvailability::Unavailable,
            ),
            (
                r#"{"availability":"restricted","is_live":true}"#,
                YouTubeAvailability::Unavailable,
            ),
            (
                r#"{"availability":"unavailable","is_live":true}"#,
                YouTubeAvailability::Unavailable,
            ),
            (
                r#"{"availability":"needs_auth","is_live":true}"#,
                YouTubeAvailability::Unavailable,
            ),
            (
                r#"{"availability":"unknown","is_live":true}"#,
                YouTubeAvailability::Unknown,
            ),
            (
                r#"{"availability":"future_status","is_live":true}"#,
                YouTubeAvailability::Unknown,
            ),
            (
                r#"{"availability":"unlisted"}"#,
                YouTubeAvailability::Unknown,
            ),
            (r#"{}"#, YouTubeAvailability::Unknown),
        ];
        for (payload, expected) in fixtures {
            let value: serde_json::Value = serde_json::from_str(payload).unwrap();
            assert_eq!(availability(&value), expected, "fixture {payload}");
        }
    }

    #[test]
    fn metadata_digest_changes_when_availability_changes() {
        let source = validate_url("https://www.youtube.com/watch?v=abc123_XY", None).unwrap();
        let public = parse_scan_plan(
            source,
            r#"{"id":"abc123_XY","title":"Video","channel":"Channel","availability":"public"}"#,
        )
        .unwrap();
        let restricted_source =
            validate_url("https://www.youtube.com/watch?v=abc123_XY", None).unwrap();
        let restricted = parse_scan_plan(
            restricted_source,
            r#"{"id":"abc123_XY","title":"Video","channel":"Channel","availability":"private"}"#,
        )
        .unwrap();

        assert_eq!(
            public.items[0].public.availability,
            YouTubeAvailability::Available
        );
        assert_eq!(
            restricted.items[0].public.availability,
            YouTubeAvailability::Unavailable
        );
        assert_ne!(
            public.items[0].public.metadata_digest,
            restricted.items[0].public.metadata_digest
        );
    }

    #[test]
    fn selected_source_revalidation_allows_display_drift_but_rejects_identity_or_availability_changes(
    ) {
        let source = validate_url("https://www.youtube.com/playlist?list=PL_12345", None).unwrap();
        let frozen = parse_scan_plan(
            source,
            r#"{"id":"PL_12345","title":"Playlist","entries":[{"id":"abc123_XY","title":"Original","channel":"Channel","availability":"public"}]}"#,
        )
        .unwrap();
        let selected = vec![frozen.items[0].public.occurrence_id.clone()];

        let current_source =
            validate_url("https://www.youtube.com/playlist?list=PL_12345", None).unwrap();
        let current = parse_scan_plan(
            current_source,
            r#"{"id":"PL_12345","title":"Renamed playlist","entries":[{"id":"abc123_XY","title":"Renamed","channel":"Channel","availability":"public"}]}"#,
        )
        .unwrap();
        let refreshed = revalidate_selected_items(&frozen, &current, &selected).unwrap();
        assert_eq!(refreshed[0].public.title, "Renamed");

        let restricted_source =
            validate_url("https://www.youtube.com/playlist?list=PL_12345", None).unwrap();
        let restricted = parse_scan_plan(
            restricted_source,
            r#"{"id":"PL_12345","entries":[{"id":"abc123_XY","title":"Renamed","availability":"private"}]}"#,
        )
        .unwrap();
        assert!(matches!(
            revalidate_selected_items(&frozen, &restricted, &selected),
            Err(YouTubeInternalError::ScanPlanStale)
        ));

        let shifted_source =
            validate_url("https://www.youtube.com/playlist?list=PL_12345", None).unwrap();
        let shifted = parse_scan_plan(
            shifted_source,
            r#"{"id":"PL_12345","entries":[{"id":"def456_ZZ","title":"Inserted","availability":"public"},{"id":"abc123_XY","title":"Original","availability":"public"}]}"#,
        )
        .unwrap();
        assert!(matches!(
            revalidate_selected_items(&frozen, &shifted, &selected),
            Err(YouTubeInternalError::ScanPlanStale)
        ));
    }
}
