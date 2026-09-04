//! `Radar` -> xbar/SwiftBar plugin text. Pure: no I/O, no clock, no environment.
//!
//! Ported from the Python `petridish.menubar`. It stays in this crate rather
//! than in `petridish-core` because it has exactly one consumer, and because
//! its labels are *not* shared vocabulary: petri renders these same buckets as
//! `RUNNING`/`IN FLIGHT`/`STALE`/`COLD` and the Raycast extension as
//! `Active`/`In Flight`/`Stale`/`Cold Storage`. Hoisting a "shared" label table
//! into core would invent an agreement that does not exist.
//!
//! Output shape — sections joined with `\n`:
//!
//! 1. Title line: `🧫 {working}/{total}`.
//! 2. **Live-session section**, flat and unindented: every project whose agent
//!    is `working`, name-sorted, whatever bucket it is in. These are the
//!    sessions running *right now*, so they sit at the top level of the
//!    dropdown instead of being buried in a bucket.
//! 3. Bucket sections in `BUCKET_ORDER`, each a header plus one `--`-indented
//!    line per project, name-sorted, **excluding** anything already shown
//!    above so nothing appears twice. A `---` divider separates the two when
//!    both are non-empty.
//! 4. Divider and a manual refresh link.
//!
//! An empty radar keeps the same shape with a `No projects` placeholder.

use petridish_core::schema::{AgentActivity, Project, Radar, StatusBucket};

/// Buckets in render order, with the labels this frontend uses.
const BUCKET_SECTIONS: [(StatusBucket, &str); 4] = [
    (StatusBucket::Active, "Active"),
    (StatusBucket::InFlight, "In flight"),
    (StatusBucket::Stale, "Stale"),
    (StatusBucket::Cold, "Cold"),
];

/// Last path component of `parent_path`, for worktree rollup display.
fn parent_basename(parent_path: &str) -> &str {
    parent_path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(parent_path)
}

/// One project's label, components concatenated in this exact order:
/// `{parent} / ` when the project is a worktree, the name, then optional
/// ` · branch`, ` ✎N` (only when dirty **and** there are uncommitted files),
/// and ` ●` (only when the agent is working). Each optional piece carries its
/// own leading separator.
fn project_label(project: &Project) -> String {
    let mut out = String::new();
    if let Some(parent) = project.parent_path.as_deref() {
        out.push_str(parent_basename(parent));
        out.push_str(" / ");
    }
    out.push_str(&project.name);

    if let Some(branch) = project.git.branch.as_deref()
        && project.git.is_repo
        && !branch.is_empty()
    {
        out.push_str(" · ");
        out.push_str(branch);
    }

    if project.git.is_dirty && project.git.uncommitted_files > 0 {
        out.push_str(" ✎");
        out.push_str(&project.git.uncommitted_files.to_string());
    }

    if project.agent.state == AgentActivity::Working {
        out.push_str(" ●");
    }

    out
}

/// One rendered menu line.
///
/// The `href` value **must** stay double-quoted. xbar splits a line's
/// `key=value` parameters on whitespace, so an unquoted path containing a
/// space — `~/Downloads/Kubernetes handin_639180485` is the real one that
/// found this — makes xbar report "malformed parameters: missing equals" and
/// disable the plugin outright.
fn project_line(project: &Project, indent: bool) -> String {
    format!(
        "{}{} | href=\"file://{}\"",
        if indent { "--" } else { "" },
        project_label(project),
        project.path
    )
}

fn by_name(mut projects: Vec<&Project>) -> Vec<&Project> {
    projects.sort_by(|a, b| a.name.cmp(&b.name));
    projects
}

pub fn render_menubar(radar: &Radar) -> String {
    let total = radar.projects.len();
    let working: Vec<&Project> = radar
        .projects
        .iter()
        .filter(|p| p.agent.state == AgentActivity::Working)
        .collect();

    let mut lines: Vec<String> = vec![format!("🧫 {}/{}", working.len(), total), "---".into()];

    if total == 0 {
        lines.push("No projects | color=#888888".into());
    } else {
        let live = by_name(working);
        for project in &live {
            lines.push(project_line(project, false));
        }

        let live_ids: Vec<&str> = live.iter().map(|p| p.id.as_str()).collect();
        let mut bucket_lines: Vec<String> = Vec::new();
        for (bucket, label) in BUCKET_SECTIONS {
            let members = by_name(
                radar
                    .projects
                    .iter()
                    .filter(|p| p.status_bucket == bucket && !live_ids.contains(&p.id.as_str()))
                    .collect(),
            );
            if members.is_empty() {
                continue;
            }
            bucket_lines.push(label.to_string());
            for project in members {
                bucket_lines.push(project_line(project, true));
            }
        }

        if !live.is_empty() && !bucket_lines.is_empty() {
            lines.push("---".into());
        }
        lines.extend(bucket_lines);
    }

    lines.push("---".into());
    lines.push("Refresh | refresh=true".into());
    lines.join("\n")
}

/// What `petridish menubar` prints when the state file is missing or
/// unreadable. Must still be valid plugin text and must still exit 0 — xbar
/// disables a plugin that errors, so a bad state file has to degrade visibly
/// rather than silently.
pub fn render_unavailable(state_path: &str) -> String {
    format!(
        "🧫 ?/?\n---\nprojects.json missing or unreadable ({state_path}) | color=#888888\n---\nRefresh | refresh=true"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use petridish_core::schema::{AgentState, GitState};

    fn ts(s: &str) -> Option<DateTime<Utc>> {
        Some(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc))
    }

    fn project(id: &str, name: &str, bucket: StatusBucket) -> Project {
        Project {
            id: id.into(),
            name: name.into(),
            path: format!("/Users/x/repos/{name}"),
            category: "repos".into(),
            parent_path: None,
            is_foreign: false,
            git: GitState {
                is_repo: true,
                branch: Some("main".into()),
                is_dirty: false,
                uncommitted_files: 0,
                last_commit_at: ts("2026-09-01T10:00:00Z"),
                mine_last_commit_at: ts("2026-09-01T10:00:00Z"),
                github_url: None,
                daily_commits: vec![],
            },
            agent: AgentState::idle_unknown(),
            last_activity_at: ts("2026-09-01T10:00:00Z"),
            status_bucket: bucket,
            agent_activity: vec![],
        }
    }

    fn working(mut p: Project) -> Project {
        p.agent.state = AgentActivity::Working;
        p
    }

    fn radar(projects: Vec<Project>) -> Radar {
        Radar {
            schema_version: 1,
            updated_at: ts("2026-09-04T12:00:00Z").unwrap(),
            scan_duration_ms: 12,
            projects,
            quota: None,
        }
    }

    #[test]
    fn an_empty_radar_still_renders_a_well_formed_menu() {
        assert_eq!(
            render_menubar(&radar(vec![])),
            "🧫 0/0\n---\nNo projects | color=#888888\n---\nRefresh | refresh=true"
        );
    }

    #[test]
    fn buckets_render_in_order_with_indented_members() {
        let out = render_menubar(&radar(vec![
            project("c", "cold-one", StatusBucket::Cold),
            project("a", "active-one", StatusBucket::Active),
            project("s", "stale-one", StatusBucket::Stale),
            project("i", "inflight-one", StatusBucket::InFlight),
        ]));
        assert_eq!(
            out,
            concat!(
                "🧫 0/4\n",
                "---\n",
                "Active\n",
                "--active-one · main | href=\"file:///Users/x/repos/active-one\"\n",
                "In flight\n",
                "--inflight-one · main | href=\"file:///Users/x/repos/inflight-one\"\n",
                "Stale\n",
                "--stale-one · main | href=\"file:///Users/x/repos/stale-one\"\n",
                "Cold\n",
                "--cold-one · main | href=\"file:///Users/x/repos/cold-one\"\n",
                "---\n",
                "Refresh | refresh=true"
            )
        );
    }

    #[test]
    fn projects_within_a_bucket_are_name_sorted() {
        let out = render_menubar(&radar(vec![
            project("1", "zebra", StatusBucket::Active),
            project("2", "alpha", StatusBucket::Active),
            project("3", "middle", StatusBucket::Active),
        ]));
        let names: Vec<&str> = out
            .lines()
            .filter(|l| l.starts_with("--") && *l != "---")
            .map(|l| l.trim_start_matches("--").split(' ').next().unwrap())
            .collect();
        assert_eq!(names, vec!["alpha", "middle", "zebra"]);
    }

    #[test]
    fn a_working_project_is_surfaced_at_top_level_and_not_repeated_in_its_bucket() {
        let out = render_menubar(&radar(vec![
            working(project("w", "live-one", StatusBucket::Cold)),
            project("o", "other-one", StatusBucket::Cold),
        ]));
        assert_eq!(
            out,
            concat!(
                "🧫 1/2\n",
                "---\n",
                "live-one · main ● | href=\"file:///Users/x/repos/live-one\"\n",
                "---\n",
                "Cold\n",
                "--other-one · main | href=\"file:///Users/x/repos/other-one\"\n",
                "---\n",
                "Refresh | refresh=true"
            )
        );
        assert_eq!(
            out.lines().filter(|l| l.contains("live-one")).count(),
            1,
            "the live project must render on exactly one line, not also under Cold"
        );
    }

    #[test]
    fn with_every_project_live_there_are_no_bucket_sections_and_no_extra_divider() {
        let out = render_menubar(&radar(vec![working(project(
            "w",
            "only",
            StatusBucket::Active,
        ))]));
        assert_eq!(
            out,
            "🧫 1/1\n---\nonly · main ● | href=\"file:///Users/x/repos/only\"\n---\nRefresh | refresh=true"
        );
    }

    #[test]
    fn the_dirty_marker_needs_both_the_flag_and_a_nonzero_count() {
        let mut clean_but_flagged = project("a", "p", StatusBucket::Active);
        clean_but_flagged.git.is_dirty = true;
        clean_but_flagged.git.uncommitted_files = 0;
        assert!(!render_menubar(&radar(vec![clean_but_flagged])).contains('✎'));

        let mut dirty = project("a", "p", StatusBucket::Active);
        dirty.git.is_dirty = true;
        dirty.git.uncommitted_files = 3;
        assert!(render_menubar(&radar(vec![dirty])).contains(" ✎3"));
    }

    #[test]
    fn a_non_repo_shows_no_branch() {
        let mut p = project("a", "notes", StatusBucket::Cold);
        p.git.is_repo = false;
        p.git.branch = None;
        let out = render_menubar(&radar(vec![p]));
        assert!(out.contains("--notes | href="), "{out}");
        assert!(!out.contains(" · "));
    }

    /// The bug this guards is xbar's, not ours: it splits `key=value`
    /// parameters on whitespace, so an unquoted path with a space in it
    /// disables the whole plugin.
    #[test]
    fn a_path_containing_spaces_stays_inside_double_quotes() {
        let mut p = project("a", "handin", StatusBucket::Cold);
        p.path = "/Users/x/Downloads/Kubernetes handin_639180485".into();
        let out = render_menubar(&radar(vec![p]));
        assert!(
            out.contains("href=\"file:///Users/x/Downloads/Kubernetes handin_639180485\""),
            "{out}"
        );
    }

    #[test]
    fn a_worktree_is_labelled_with_its_parent() {
        let mut p = project("a", "feature-x", StatusBucket::Active);
        p.parent_path = Some("/Users/x/repos/mainline".into());
        assert!(render_menubar(&radar(vec![p])).contains("--mainline / feature-x · main | href="),);
    }

    #[test]
    fn a_trailing_slash_on_the_parent_path_does_not_produce_an_empty_label() {
        let mut p = project("a", "feature-x", StatusBucket::Active);
        p.parent_path = Some("/Users/x/repos/mainline/".into());
        assert!(render_menubar(&radar(vec![p])).contains("mainline / feature-x"));
    }

    #[test]
    fn the_unavailable_block_is_still_valid_plugin_text() {
        let out = render_unavailable("/Users/x/.petridish/projects.json");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "🧫 ?/?");
        assert_eq!(lines[4], "Refresh | refresh=true");
    }
}
