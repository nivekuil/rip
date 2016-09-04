// -*- compile-command: "cargo build" -*-
#[macro_use]
extern crate clap;
extern crate walkdir;

use clap::{Arg, App};
use walkdir::WalkDir;
use std::path::{Path, PathBuf};
use std::fs;
use std::env;

static GRAVEYARD: &'static str = "/tmp/.graveyard";

fn main() {
    let matches = App::with_defaults("rip")
        .version(crate_version!())
        .author(crate_authors!())
        .about("Rm ImProved
Send files to the graveyard (/tmp/.graveyard) instead of unlinking them.")
        .arg(Arg::with_name("SOURCE")
             .help("File or directory to remove")
             .required(true)
             .multiple(true)
             .index(1)
             .conflicts_with("decompose")
             .conflicts_with("seance"))
        .arg(Arg::with_name("graveyard")
             .help("Directory where deleted files go to rest")
             .long("graveyard")
             .takes_value(true))
        .arg(Arg::with_name("decompose")
             .help("Permanently deletes (unlink) the entire graveyard")
             .long("decompose"))
        .arg(Arg::with_name("seance")
             .help("List all objects in the graveyard that were sent from \
                    the current directory")
             .short("s")
             .long("seance"))
        .get_matches();

    let graveyard: &Path = Path::new(matches.value_of("graveyard")
                                     .unwrap_or(GRAVEYARD));

    if matches.is_present("decompose") {
        fs::remove_dir_all(graveyard).expect("Failed to delete graveyard");
        return;
    }

    let cwd: PathBuf = env::current_dir().expect("Failed to get current dir");
    // Can't join absolute paths, so we need to strip the leading "/"
    let cwd: &Path = cwd.strip_prefix("/").expect("cwd doesn't have a root?");
    if matches.is_present("seance") {
        for entry in WalkDir::new(graveyard.join(cwd)).into_iter().skip(1) {
            println!("{}", entry.unwrap().path().display());
        }
        return;
    }

    if cwd.starts_with(graveyard) {
        println!("You should use rm to delete files in the graveyard, \
                  or --decompose to delete everything at once.");
        return;
    }

    let sources: clap::Values = matches.values_of("SOURCE").unwrap();
    for source in sources {
        if let Err(e) = bury(source, &cwd, graveyard) {
            println!("ERROR: {}: {}", e, source);
        }
    }
}

fn bury(source: &str, cwd: &Path, graveyard: &Path) -> std::io::Result<()> {
    let fullpath: PathBuf = cwd.join(Path::new(source));
    let dest: PathBuf = {
        let grave = graveyard.join(&fullpath);
        // Avoid a name conflict if necessary.
        if grave.exists() {
            // println!("found name conflict {}", grave.display());
            numbered_rename(&grave)
        } else {
            grave
        }
    };

    // Try a simple rename, which will only work within the same mount point.
    // Trying to rename across filesystems will throw errno 18.
    if let Ok(_) = fs::rename(source, &dest) {
        return Ok(());
    }

    // If that didn't work, then copy and rm.
    if fullpath.is_dir() {
        // Create all directories including the top-level dir, and then
        // skip the top-level dir in WalkDir because it may be renamed
        // due to name collision
        fs::create_dir_all(&dest).expect("Failed to create grave path");

        // Walk the source, creating directories and copying files as needed
        for entry in WalkDir::new(source).into_iter().skip(1) {
            let entry = entry.expect("Failed to open file in source dir");
            let path: &Path = entry.path();
            let orphan: &Path = path.strip_prefix(source)
                .expect("Failed to descend into directory");
            if path.is_dir() {
                if let Err(e) = fs::create_dir(dest.join(orphan)) {
                    println!("Failed to create {} in {}",
                             path.display(),
                             dest.join(orphan).display());
                    fs::remove_dir_all(&dest).unwrap();
                    return Err(e);
                };
            } else {
                if let Err(e) = fs::copy(path, dest.join(orphan)) {
                    println!("Failed to copy {} to {}",
                             path.display(),
                             dest.join(orphan).display());
                    fs::remove_dir_all(&dest).unwrap();
                    return Err(e);
                };
            }
        }
        fs::remove_dir_all(source).expect("Failed to remove source dir");
    } else {
        fs::create_dir_all(dest.parent().unwrap())
            .expect("Failed to create grave path");
        if let Err(e) = fs::copy(source, &dest) {
            println!("Failed to copy {} to {}", source, dest.display());
            return Err(e);
        }
        if let Err(e) = fs::remove_file(source) {
            println!("Failed to remove {}", source);
            return Err(e);
        };
    }

    Ok(())
}

fn numbered_rename(path: &PathBuf) -> PathBuf {
    (1_u64..)
        .map(|i| path.with_extension(format!("~{}~", i)))
        .skip_while(|p| p.exists())
        .next()
        .expect("Failed to rename duplicate file or directory")
}
