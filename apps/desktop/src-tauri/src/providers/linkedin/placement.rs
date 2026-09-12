//! Disk home for a LinkedIn course under one user output root.
//! Catalog membership is many-to-many. Placement is one-to-one.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

use super::expansion::PathCapture;
use super::path_library::{CourseSlug, PathSlug};

/// User-selected destination. Already validated as a regular directory
/// (junctions / reparse points rejected by `validate_output_root`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputRoot(String);

impl OutputRoot {
    pub fn parse(raw: &str) -> Result<Self, PlacementError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(PlacementError::MissingOutputRoot);
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Frozen folder segment. Never contains `\`, `/`, `..`, or Windows reserved chars.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LayoutName(String);

impl LayoutName {
    pub fn from_title(title: &str) -> Self {
        Self(sanitize_layout_segment(title))
    }

    pub fn with_slug_disambiguator(base: &Self, path_slug: &PathSlug) -> Self {
        Self::from_title(&format!("{} ({})", base.as_str(), path_slug.as_str()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CourseHome {
    Standalone,
    Certificate {
        /// None when folder-import recovered a nested course without a path slug.
        path: Option<PathSlug>,
        layout_name: LayoutName,
    },
}

impl CourseHome {
    pub fn kind_key(&self) -> &'static str {
        match self {
            Self::Standalone => "standalone",
            Self::Certificate { .. } => "certificate",
        }
    }
}

/// Relative segments under `jobs.output_dir` for one course's files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CourseLayout {
    output_root: OutputRoot,
    home: CourseHome,
}

impl CourseLayout {
    #[cfg(test)]
    pub fn standalone(output_root: OutputRoot) -> Self {
        Self {
            output_root,
            home: CourseHome::Standalone,
        }
    }

    /// Read the frozen placement row. A missing row freezes Standalone so a
    /// later membership capture cannot nest a previously-flat job.
    pub fn load(
        conn: &Connection,
        output_root: &str,
        course_slug: &str,
    ) -> Result<Self, PlacementError> {
        let output_root = OutputRoot::parse(output_root)?;
        if let Some(home) = select_home(conn, output_root.as_str(), course_slug)? {
            return Ok(Self { output_root, home });
        }
        insert_standalone_ignore(conn, output_root.as_str(), course_slug, unix_timestamp())?;
        let home =
            select_home(conn, output_root.as_str(), course_slug)?.unwrap_or(CourseHome::Standalone);
        Ok(Self { output_root, home })
    }

    pub fn prefix_segments(&self) -> Vec<&str> {
        match &self.home {
            CourseHome::Standalone => Vec::new(),
            CourseHome::Certificate { layout_name, .. } => vec![layout_name.as_str()],
        }
    }

    pub fn course_folder(&self, course_title: &str) -> LayoutName {
        LayoutName::from_title(course_title)
    }

    pub fn course_dir(&self, course_title: &str) -> PathBuf {
        let mut path = PathBuf::from(self.output_root.as_str());
        for segment in self.prefix_segments() {
            path.push(segment);
        }
        path.push(self.course_folder(course_title).as_str());
        path
    }

    pub fn output_root(&self) -> &OutputRoot {
        &self.output_root
    }

    #[cfg(test)]
    pub fn home(&self) -> &CourseHome {
        &self.home
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlacementError {
    #[error("LinkedIn output folder is missing")]
    MissingOutputRoot,
    #[error("invalid LinkedIn slug '{value}'")]
    InvalidSlug { value: String },
    #[error("linkedin course placement row is invalid for '{course_slug}'")]
    InvalidPlacement { course_slug: String },
    #[error(transparent)]
    Database(#[from] crate::app::database_writer::DatabaseWriteError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

pub struct PlacementBatch {
    pub path_layout_names: Vec<(PathSlug, LayoutName)>,
    pub homes: Vec<(CourseSlug, CourseHome)>,
}

/// First insert wins. Walk `paths[]` in expansion order. Lexicographic
/// `path_slug` is only a fallback when that order is unavailable.
pub fn plan_first_writer_homes(
    paths: &[PathCapture],
    standalone: &[CourseSlug],
    existing: &HashMap<CourseSlug, CourseHome>,
    taken_layout_names: &HashSet<String>,
    frozen_path_names: &HashMap<PathSlug, LayoutName>,
) -> Result<PlacementBatch, PlacementError> {
    let mut taken = taken_layout_names.clone();
    let mut homes = Vec::new();
    let mut queued = HashSet::new();
    let mut path_names = Vec::new();

    for capture in paths {
        let path_slug = parse_path_slug(&capture.path_slug)?;
        let name = if let Some(frozen) = frozen_path_names.get(&path_slug) {
            taken.insert(frozen.as_str().to_string());
            frozen.clone()
        } else {
            allocate_layout_name(&capture.title, &path_slug, &mut taken)
        };
        path_names.push((path_slug.clone(), name.clone()));
        for member in &capture.members {
            let course = parse_course_slug(member)?;
            if existing.contains_key(&course) || queued.contains(&course) {
                continue;
            }
            queued.insert(course.clone());
            homes.push((
                course,
                CourseHome::Certificate {
                    path: Some(path_slug.clone()),
                    layout_name: name.clone(),
                },
            ));
        }
    }

    for slug in standalone {
        if existing.contains_key(slug) || queued.contains(slug) {
            continue;
        }
        queued.insert(slug.clone());
        homes.push((slug.clone(), CourseHome::Standalone));
    }

    Ok(PlacementBatch {
        path_layout_names: path_names,
        homes,
    })
}

fn allocate_layout_name(
    title: &str,
    path_slug: &PathSlug,
    taken: &mut HashSet<String>,
) -> LayoutName {
    let base = LayoutName::from_title(title);
    if taken.insert(base.as_str().to_string()) {
        return base;
    }
    let disambiguated = LayoutName::with_slug_disambiguator(&base, path_slug);
    taken.insert(disambiguated.as_str().to_string());
    disambiguated
}

pub(crate) fn load_placements(
    conn: &Connection,
    output_root: &OutputRoot,
) -> Result<HashMap<CourseSlug, CourseHome>, PlacementError> {
    let mut statement = conn.prepare(
        "SELECT course_slug, home_kind, path_slug, layout_name
         FROM linkedin_course_placement
         WHERE output_root = ?1",
    )?;
    let rows = statement.query_map(params![output_root.as_str()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    let mut homes = HashMap::new();
    for row in rows {
        let (course_slug, home_kind, path_slug, layout_name) = row?;
        let home = course_home_from_row(&course_slug, &home_kind, path_slug, layout_name)?;
        homes.insert(parse_course_slug(&course_slug)?, home);
    }
    Ok(homes)
}

pub(crate) fn load_layout_names(
    conn: &Connection,
    output_root: &OutputRoot,
) -> Result<HashSet<String>, PlacementError> {
    let mut statement = conn.prepare(
        "SELECT layout_name FROM linkedin_course_placement
         WHERE output_root = ?1 AND layout_name IS NOT NULL",
    )?;
    let rows = statement.query_map(params![output_root.as_str()], |row| row.get::<_, String>(0))?;
    let mut names = HashSet::new();
    for name in rows {
        names.insert(name?);
    }
    Ok(names)
}

pub(crate) fn load_frozen_path_names(
    conn: &Connection,
) -> Result<HashMap<PathSlug, LayoutName>, PlacementError> {
    let mut statement = conn.prepare(
        "SELECT path_slug, layout_name FROM linkedin_learning_paths
         WHERE layout_name IS NOT NULL AND TRIM(layout_name) != ''",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut names = HashMap::new();
    for row in rows {
        let (path_slug, layout_name) = row?;
        names.insert(parse_path_slug(&path_slug)?, LayoutName(layout_name));
    }
    Ok(names)
}

pub(crate) fn insert_placement_ignore(
    conn: &Connection,
    output_root: &OutputRoot,
    course: &CourseSlug,
    home: &CourseHome,
    now: i64,
) -> Result<(), rusqlite::Error> {
    let (path_slug, layout_name) = match home {
        CourseHome::Standalone => (None, None),
        CourseHome::Certificate { path, layout_name } => (
            path.as_ref().map(PathSlug::as_str),
            Some(layout_name.as_str()),
        ),
    };
    conn.execute(
        "INSERT OR IGNORE INTO linkedin_course_placement (
            course_slug, output_root, home_kind, path_slug, layout_name, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            course.as_str(),
            output_root.as_str(),
            home.kind_key(),
            path_slug,
            layout_name,
            now
        ],
    )?;
    Ok(())
}

fn select_home(
    conn: &Connection,
    output_root: &str,
    course_slug: &str,
) -> Result<Option<CourseHome>, PlacementError> {
    let row: Option<(String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT home_kind, path_slug, layout_name
             FROM linkedin_course_placement
             WHERE course_slug = ?1 AND output_root = ?2",
            params![course_slug, output_root],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    match row {
        Some((home_kind, path_slug, layout_name)) => Ok(Some(course_home_from_row(
            course_slug,
            &home_kind,
            path_slug,
            layout_name,
        )?)),
        None => Ok(None),
    }
}

fn insert_standalone_ignore(
    conn: &Connection,
    output_root: &str,
    course_slug: &str,
    now: i64,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR IGNORE INTO linkedin_course_placement (
            course_slug, output_root, home_kind, path_slug, layout_name, created_at
         ) VALUES (?1, ?2, 'standalone', NULL, NULL, ?3)",
        params![course_slug, output_root, now],
    )?;
    Ok(())
}

fn course_home_from_row(
    course_slug: &str,
    home_kind: &str,
    path_slug: Option<String>,
    layout_name: Option<String>,
) -> Result<CourseHome, PlacementError> {
    match home_kind {
        "standalone" => {
            if path_slug.is_some() || layout_name.is_some() {
                return Err(PlacementError::InvalidPlacement {
                    course_slug: course_slug.to_string(),
                });
            }
            Ok(CourseHome::Standalone)
        }
        "certificate" => {
            let layout_name = layout_name.filter(|value| !value.trim().is_empty()).ok_or(
                PlacementError::InvalidPlacement {
                    course_slug: course_slug.to_string(),
                },
            )?;
            let path = match path_slug {
                Some(value) if !value.trim().is_empty() => Some(parse_path_slug(&value)?),
                _ => None,
            };
            Ok(CourseHome::Certificate {
                path,
                layout_name: LayoutName(layout_name),
            })
        }
        _ => Err(PlacementError::InvalidPlacement {
            course_slug: course_slug.to_string(),
        }),
    }
}

fn parse_path_slug(value: &str) -> Result<PathSlug, PlacementError> {
    PathSlug::parse(value).map_err(placement_slug_error)
}

fn parse_course_slug(value: &str) -> Result<CourseSlug, PlacementError> {
    CourseSlug::parse(value).map_err(placement_slug_error)
}

fn placement_slug_error(error: super::path_library::PathLibraryError) -> PlacementError {
    match error {
        super::path_library::PathLibraryError::InvalidSlug { value } => {
            PlacementError::InvalidSlug { value }
        }
        other => PlacementError::InvalidSlug {
            value: other.to_string(),
        },
    }
}

fn sanitize_layout_segment(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            ) || character.is_control()
            {
                '-'
            } else {
                character
            }
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string();
    if sanitized.is_empty() {
        "download".to_string()
    } else {
        sanitized
    }
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(slug: &str, title: &str, members: &[&str]) -> PathCapture {
        PathCapture {
            path_slug: slug.to_string(),
            title: title.to_string(),
            source_url: format!("https://www.linkedin.com/learning/paths/{slug}"),
            members: members.iter().map(|member| (*member).to_string()).collect(),
        }
    }

    fn course(slug: &str) -> CourseSlug {
        CourseSlug::parse(slug).unwrap()
    }

    fn path(slug: &str) -> PathSlug {
        PathSlug::parse(slug).unwrap()
    }

    fn certificate_home<'a>(homes: &'a PlacementBatch, slug: &str) -> &'a CourseHome {
        homes
            .homes
            .iter()
            .find(|(course_slug, _)| course_slug.as_str() == slug)
            .map(|(_, home)| home)
            .unwrap_or_else(|| panic!("missing home for {slug}"))
    }

    #[test]
    fn first_writer_same_paste_two_paths_keeps_first_path_home() {
        let paths = vec![
            capture(
                "zeta-cert",
                "Zeta Certificate",
                &["shared-course", "only-z"],
            ),
            capture(
                "alpha-cert",
                "Alpha Certificate",
                &["shared-course", "only-a"],
            ),
        ];
        let batch = plan_first_writer_homes(
            &paths,
            &[],
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
        )
        .unwrap();

        match certificate_home(&batch, "shared-course") {
            CourseHome::Certificate {
                path: Some(path_slug),
                layout_name,
            } => {
                assert_eq!(path_slug.as_str(), "zeta-cert");
                assert_eq!(layout_name.as_str(), "Zeta Certificate");
            }
            other => panic!("expected first-path certificate home, got {other:?}"),
        }
        match certificate_home(&batch, "only-a") {
            CourseHome::Certificate {
                path: Some(path_slug),
                ..
            } => assert_eq!(path_slug.as_str(), "alpha-cert"),
            other => panic!("expected alpha home, got {other:?}"),
        }
        assert_eq!(batch.homes.len(), 3);
    }

    #[test]
    fn second_path_later_does_not_move_home_or_duplicate_job() {
        let mut existing = HashMap::new();
        existing.insert(
            course("shared-course"),
            CourseHome::Certificate {
                path: Some(path("first-cert")),
                layout_name: LayoutName::from_title("First Certificate"),
            },
        );
        let batch = plan_first_writer_homes(
            &[capture(
                "second-cert",
                "Second Certificate",
                &["shared-course", "only-second"],
            )],
            &[],
            &existing,
            &HashSet::from([String::from("First Certificate")]),
            &HashMap::new(),
        )
        .unwrap();

        assert!(batch
            .homes
            .iter()
            .all(|(slug, _)| slug.as_str() != "shared-course"));
        match certificate_home(&batch, "only-second") {
            CourseHome::Certificate {
                path: Some(path_slug),
                layout_name,
            } => {
                assert_eq!(path_slug.as_str(), "second-cert");
                assert_eq!(layout_name.as_str(), "Second Certificate");
            }
            other => panic!("expected new member under second path, got {other:?}"),
        }
    }

    #[test]
    fn second_path_membership_does_not_relocate_home() {
        second_path_later_does_not_move_home_or_duplicate_job();
    }

    #[test]
    fn standalone_first_then_path_stays_standalone() {
        let mut existing = HashMap::new();
        existing.insert(course("already-flat"), CourseHome::Standalone);
        let batch = plan_first_writer_homes(
            &[capture(
                "later-cert",
                "Later Certificate",
                &["already-flat", "new-member"],
            )],
            &[],
            &existing,
            &HashSet::new(),
            &HashMap::new(),
        )
        .unwrap();

        assert!(batch
            .homes
            .iter()
            .all(|(slug, _)| slug.as_str() != "already-flat"));
        assert!(matches!(
            certificate_home(&batch, "new-member"),
            CourseHome::Certificate { .. }
        ));
    }

    #[test]
    fn standalone_home_before_path_membership_stays_flat() {
        standalone_first_then_path_stays_standalone();
    }

    #[test]
    fn path_first_then_standalone_paste_keeps_certificate_home() {
        let mut existing = HashMap::new();
        existing.insert(
            course("nested-course"),
            CourseHome::Certificate {
                path: Some(path("github-cert")),
                layout_name: LayoutName::from_title("GitHub Certificate"),
            },
        );
        let batch = plan_first_writer_homes(
            &[],
            &[course("nested-course"), course("brand-new")],
            &existing,
            &HashSet::from([String::from("GitHub Certificate")]),
            &HashMap::new(),
        )
        .unwrap();

        assert!(batch
            .homes
            .iter()
            .all(|(slug, _)| slug.as_str() != "nested-course"));
        assert!(matches!(
            certificate_home(&batch, "brand-new"),
            CourseHome::Standalone
        ));
    }

    #[test]
    fn leftover_standalone_slugs_sit_at_user_root() {
        let batch = plan_first_writer_homes(
            &[capture(
                "github-cert",
                "GitHub Certificate",
                &["path-member"],
            )],
            &[course("path-member"), course("solo-course")],
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
        )
        .unwrap();

        assert!(matches!(
            certificate_home(&batch, "path-member"),
            CourseHome::Certificate { .. }
        ));
        assert!(matches!(
            certificate_home(&batch, "solo-course"),
            CourseHome::Standalone
        ));
    }

    #[test]
    fn layout_name_collision_appends_path_slug() {
        let taken = HashSet::from([String::from("Career Essentials")]);
        let batch = plan_first_writer_homes(
            &[capture(
                "career-essentials-in-github-professional-certificate",
                "Career Essentials",
                &["practical-github-actions"],
            )],
            &[],
            &HashMap::new(),
            &taken,
            &HashMap::new(),
        )
        .unwrap();

        assert_eq!(
            batch.path_layout_names[0].1.as_str(),
            "Career Essentials (career-essentials-in-github-professional-certificate)"
        );
    }

    #[test]
    fn frozen_path_layout_name_is_reused_after_title_change() {
        let mut frozen = HashMap::new();
        frozen.insert(
            path("github-cert"),
            LayoutName::from_title("Original Certificate"),
        );
        let batch = plan_first_writer_homes(
            &[capture(
                "github-cert",
                "Renamed Certificate Title",
                &["new-member"],
            )],
            &[],
            &HashMap::new(),
            &HashSet::from([String::from("Original Certificate")]),
            &frozen,
        )
        .unwrap();

        match certificate_home(&batch, "new-member") {
            CourseHome::Certificate { layout_name, .. } => {
                assert_eq!(layout_name.as_str(), "Original Certificate");
            }
            other => panic!("expected frozen layout name, got {other:?}"),
        }
    }
}
