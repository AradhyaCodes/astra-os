//! Compatibility helpers used by the Phase 0 Almanac executor.
//!
//! The filesystem engine itself lives on [`VirtualFileSystem`]. These
//! functions keep the existing shell commands working while Almanac's full
//! command grammar is developed in a later phase.

use crate::{error::AaruError, filesystem::model::VirtualFileSystem};

type Result<T> = std::result::Result<T, AaruError>;

pub fn mkdir(vfs: &mut VirtualFileSystem, cwd: &str, path: &str) -> Result<()> {
    vfs.create_directory(cwd, path).map(|_| ())
}

pub fn touch(vfs: &mut VirtualFileSystem, cwd: &str, path: &str) -> Result<()> {
    vfs.create_file(cwd, path, "").map(|_| ())
}

pub fn ls(vfs: &VirtualFileSystem, cwd: &str, path: &str) -> Result<Vec<String>> {
    vfs.list_directory(cwd, path).map(|resources| {
        resources
            .into_iter()
            .map(|item| item.metadata.name)
            .collect()
    })
}

pub fn cat(vfs: &VirtualFileSystem, cwd: &str, path: &str) -> Result<String> {
    vfs.read_file(cwd, path)
}

pub fn write(vfs: &mut VirtualFileSystem, cwd: &str, path: &str, content: &str) -> Result<()> {
    vfs.write_file(cwd, path, content).map(|_| ())
}

pub fn rm(vfs: &mut VirtualFileSystem, cwd: &str, path: &str) -> Result<()> {
    vfs.delete_recursive(cwd, path).map(|_| ())
}

pub fn cd(vfs: &VirtualFileSystem, cwd: &str, path: &str) -> Result<String> {
    vfs.open_directory(cwd, path).map(|item| item.path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_zero_commands_use_the_phase_one_engine() {
        let mut vfs = VirtualFileSystem::new();
        mkdir(&mut vfs, "ROOT", "Projects>AaruOS").expect("mkdir");
        touch(&mut vfs, "ROOT", "Projects>AaruOS>notes.txt").expect("touch");
        write(&mut vfs, "ROOT", "Projects>AaruOS>notes.txt", "hello").expect("write");

        assert_eq!(
            cat(&vfs, "ROOT", "Projects>AaruOS>notes.txt").unwrap(),
            "hello"
        );
        assert_eq!(
            cd(&vfs, "ROOT", "Projects>AaruOS").unwrap(),
            "ROOT>Projects>AaruOS"
        );
    }
}
