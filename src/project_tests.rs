use super::*;
use std::fs;
use std::path::Path;

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn repo(dir: &Path) {
    fs::create_dir_all(dir.join(".git")).unwrap();
}

#[test]
fn a_subdirectory_maps_to_its_repo_root() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("app");
    repo(&root);
    fs::create_dir_all(root.join("crates/x")).unwrap();
    let p = project_for(&s(&root.join("crates/x")));
    assert_eq!(
        p,
        Project {
            key: s(&root),
            name: "app".into()
        }
    );
}

#[test]
fn a_worktree_maps_to_its_main_repo() {
    let t = tempfile::tempdir().unwrap();
    let main = t.path().join("main");
    let gitdir = main.join(".git/worktrees/wt");
    fs::create_dir_all(&gitdir).unwrap();
    fs::write(gitdir.join("commondir"), "../..\n").unwrap();
    for (name, pointer) in [
        ("wt-abs", s(&gitdir)),
        ("wt-rel", "../main/.git/worktrees/wt".to_string()),
    ] {
        let wt = t.path().join(name);
        fs::create_dir_all(wt.join("src")).unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {pointer}\n")).unwrap();
        assert_eq!(project_for(&s(&wt.join("src"))).key, s(&main), "{name}");
    }
}

#[test]
fn a_submodule_is_its_own_project() {
    let t = tempfile::tempdir().unwrap();
    let sup = t.path().join("super");
    fs::create_dir_all(sup.join(".git/modules/sub")).unwrap();
    let sub = sup.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join(".git"), "gitdir: ../.git/modules/sub\n").unwrap();
    assert_eq!(
        project_for(&s(&sub)),
        Project {
            key: s(&sub),
            name: "sub".into()
        }
    );
}

#[test]
fn outside_a_repo_the_cwd_is_the_project() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("scratch");
    fs::create_dir_all(&dir).unwrap();
    assert_eq!(
        project_for(&s(&dir)),
        Project {
            key: s(&dir),
            name: "scratch".into()
        }
    );
}

#[test]
fn a_deleted_cwd_resolves_through_its_ancestors() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("app");
    repo(&root);
    assert_eq!(
        project_for(&s(&root.join(".worktrees/gone/src"))).key,
        s(&root)
    );
}

#[test]
fn dots_in_the_cwd_are_resolved() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("app");
    repo(&root);
    fs::create_dir_all(root.join("crates")).unwrap();
    assert_eq!(
        project_for(&format!("{}/crates/../.", s(&root))).key,
        s(&root)
    );
}

#[test]
fn empty_or_relative_cwds_are_unknown() {
    for cwd in ["", "rel/path"] {
        assert_eq!(
            project_for(cwd),
            Project {
                key: cwd.into(),
                name: "unknown".into()
            }
        );
    }
}

#[test]
fn the_resolver_caches_per_cwd() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("app");
    repo(&root);
    let cwd = s(&root.join("src"));
    let mut r = Resolver::default();
    let first = r.project(&cwd);
    fs::remove_dir_all(root.join(".git")).unwrap();
    assert_eq!(r.project(&cwd), first);
    assert_ne!(project_for(&cwd), first);
}

#[test]
fn uuid_sessions_get_short_ids() {
    let uuid = "1900bae3-5c1e-4c47-9a0e-1234567890ab";
    assert!(is_uuid(uuid));
    assert_eq!(short_session(uuid), "1900bae3");
    assert_eq!(display_id(uuid, 3154), "1900bae3:3154");
    for other in [
        "kinetic-photon-flux",
        "1900bae3-5c1e-4c47-9a0e-1234567890aZ",
        "1900bae3",
    ] {
        assert!(!is_uuid(other), "{other}");
        assert_eq!(short_session(other), other);
    }
}
