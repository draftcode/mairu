/// Returns the equivalent path in the repository's main checkout when the given path is inside a
/// linked git worktree. Returns None when the path is not in a linked worktree, or when the
/// equivalent path is the given path itself.
pub fn main_checkout_equivalent(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let start = path.parent()?;
    let start_device = device(start)?;
    for worktree_root in start.ancestors() {
        if device(worktree_root) != Some(start_device) {
            return None;
        }
        let dotgit = worktree_root.join(".git");
        if dotgit.is_dir() {
            return None;
        }
        if dotgit.is_file()
            && let Some(main_root) = main_checkout_root(&dotgit)
        {
            let equivalent = main_root.join(path.strip_prefix(worktree_root).ok()?);
            return (equivalent != path).then_some(equivalent);
        }
    }
    None
}

/// Device the given path lives on. The ancestor walk stops when this changes, so that a checkout
/// mounted below an unrelated one does not reach the outer repository.
#[cfg(unix)]
fn device(path: &std::path::Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(std::fs::metadata(path).ok()?.dev())
}

#[cfg(not(unix))]
fn device(path: &std::path::Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|_| 0)
}

fn main_checkout_root(dotgit_file: &std::path::Path) -> Option<std::path::PathBuf> {
    let contents = std::fs::read_to_string(dotgit_file).ok()?;
    let gitdir = resolve(contents.strip_prefix("gitdir:")?, dotgit_file.parent()?)?;

    let commondir = resolve(
        &std::fs::read_to_string(gitdir.join("commondir")).ok()?,
        &gitdir,
    )?;
    if commondir.file_name() != Some(std::ffi::OsStr::new(".git")) {
        return None;
    }

    // Both pointers read so far live next to the worktree and are writable by whoever controls it,
    // so on their own they assert a relationship rather than prove one. Require the layout that
    // `git worktree add` actually produces: the gitdir sits in the common .git's own `worktrees`
    // directory, and the administrative `gitdir` file there points back at the .git file we came
    // from. Both of those live inside the main checkout, out of reach of a forged worktree.
    if gitdir.parent()? != commondir.join("worktrees") {
        return None;
    }
    let back_pointer = resolve(
        &std::fs::read_to_string(gitdir.join("gitdir")).ok()?,
        &gitdir,
    )?;
    if back_pointer != dotgit_file.canonicalize().ok()? {
        return None;
    }

    commondir.parent().map(|p| p.to_path_buf())
}

/// Resolves the path recorded in a git pointer file, taking relative paths as relative to the
/// given base. The result is canonicalized so that the caller can compare paths by equality.
fn resolve(pointer: &str, base: &std::path::Path) -> Option<std::path::PathBuf> {
    let path = std::path::Path::new(pointer.trim());
    if path.is_relative() {
        base.join(path)
    } else {
        path.to_path_buf()
    }
    .canonicalize()
    .ok()
}

#[cfg(test)]
mod tests {
    /// A temporary directory that is removed even when the test panics.
    struct ScratchDir(std::path::PathBuf);

    impl std::ops::Deref for ScratchDir {
        type Target = std::path::Path;

        fn deref(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch_dir(name: &str) -> ScratchDir {
        let dir = std::env::temp_dir().join(format!(
            "mairu-git-test-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        ScratchDir(dir.canonicalize().unwrap())
    }

    /// Builds `<root>/main/.git/worktrees/wt` plus `<root>/wt/.git` pointing at it, and returns
    /// (main checkout root, linked worktree root).
    fn fake_worktree(root: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
        let main_root = root.join("main");
        let gitdir = main_root.join(".git").join("worktrees").join("wt");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::write(gitdir.join("commondir"), "../..\n").unwrap();

        let worktree_root = root.join("wt");
        std::fs::create_dir_all(&worktree_root).unwrap();
        std::fs::write(
            worktree_root.join(".git"),
            format!("gitdir: {}\n", gitdir.display()),
        )
        .unwrap();
        std::fs::write(
            gitdir.join("gitdir"),
            format!("{}\n", worktree_root.join(".git").display()),
        )
        .unwrap();

        (main_root, worktree_root)
    }

    #[test]
    fn maps_linked_worktree_to_main_checkout() {
        let root = scratch_dir("linked");
        let (main_root, worktree_root) = fake_worktree(&root);
        std::fs::create_dir_all(worktree_root.join("sub")).unwrap();

        assert_eq!(
            super::main_checkout_equivalent(&worktree_root.join("sub").join(".mairu.json")),
            Some(main_root.join("sub").join(".mairu.json"))
        );
    }

    #[test]
    fn returns_none_for_main_checkout() {
        let root = scratch_dir("main");
        let (main_root, _) = fake_worktree(&root);

        assert_eq!(
            super::main_checkout_equivalent(&main_root.join(".mairu.json")),
            None
        );
    }

    #[test]
    fn returns_none_outside_repository() {
        let root = scratch_dir("bare");
        std::fs::create_dir_all(root.join("plain")).unwrap();

        assert_eq!(
            super::main_checkout_equivalent(&root.join("plain").join(".mairu.json")),
            None
        );
    }

    #[test]
    fn returns_none_when_gitdir_is_not_a_worktree() {
        let root = scratch_dir("submodule");
        let gitdir = root.join("main").join(".git").join("modules").join("sub");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::write(gitdir.join("commondir"), "../..\n").unwrap();

        let submodule_root = root.join("main").join("sub");
        std::fs::create_dir_all(&submodule_root).unwrap();
        std::fs::write(
            submodule_root.join(".git"),
            format!("gitdir: {}\n", gitdir.display()),
        )
        .unwrap();

        assert_eq!(
            super::main_checkout_equivalent(&submodule_root.join(".mairu.json")),
            None
        );
    }

    /// A directory that is not registered in the main checkout must not inherit its trust, even
    /// though every file it controls claims the relationship.
    #[test]
    fn returns_none_when_gitdir_is_outside_the_common_dir() {
        let root = scratch_dir("forged");
        let (main_root, _) = fake_worktree(&root);

        let gitdir = root.join("forged-meta").join("worktrees").join("wt");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::write(
            gitdir.join("commondir"),
            format!("{}\n", main_root.join(".git").display()),
        )
        .unwrap();

        let forged_root = root.join("forged");
        std::fs::create_dir_all(&forged_root).unwrap();
        std::fs::write(
            forged_root.join(".git"),
            format!("gitdir: {}\n", gitdir.display()),
        )
        .unwrap();
        std::fs::write(
            gitdir.join("gitdir"),
            format!("{}\n", forged_root.join(".git").display()),
        )
        .unwrap();

        assert_eq!(
            super::main_checkout_equivalent(&forged_root.join(".mairu.json")),
            None
        );
    }

    /// Pointing at a worktree the main checkout does register is not enough either: its
    /// administrative `gitdir` file names the one directory allowed to claim it.
    #[test]
    fn returns_none_when_the_registered_worktree_points_elsewhere() {
        let root = scratch_dir("hijack");
        let (main_root, _) = fake_worktree(&root);

        let hijack_root = root.join("hijack");
        std::fs::create_dir_all(&hijack_root).unwrap();
        std::fs::write(
            hijack_root.join(".git"),
            format!(
                "gitdir: {}\n",
                main_root
                    .join(".git")
                    .join("worktrees")
                    .join("wt")
                    .display()
            ),
        )
        .unwrap();

        assert_eq!(
            super::main_checkout_equivalent(&hijack_root.join(".mairu.json")),
            None
        );
    }

    #[test]
    fn supports_relative_gitdir() {
        let root = scratch_dir("relative");
        let main_root = root.join("main");
        let gitdir = main_root.join(".git").join("worktrees").join("wt");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::write(gitdir.join("commondir"), "../..\n").unwrap();

        let worktree_root = root.join("wt");
        std::fs::create_dir_all(&worktree_root).unwrap();
        std::fs::write(
            worktree_root.join(".git"),
            "gitdir: ../main/.git/worktrees/wt\n",
        )
        .unwrap();
        std::fs::write(gitdir.join("gitdir"), "../../../../wt/.git\n").unwrap();

        assert_eq!(
            super::main_checkout_equivalent(&worktree_root.join(".mairu.json")),
            Some(main_root.join(".mairu.json"))
        );
    }
}
