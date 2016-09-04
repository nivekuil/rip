// -*- compile-command: "cargo build" -*-
#[macro_use]
extern crate clap;
extern crate walkdir;

use clap::{Arg, App};
use walkdir::WalkDir;
use std::path::{Path, PathBuf};
use std::fs;
use std::env::current_dir;

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
            .index(1))
        .arg(Arg::with_name("graveyard")
            .help("Directory where deleted files go to rest")
            .long("graveyard")
            .takes_value(true))
        .get_matches();

    let graveyard: &Path = Path::new(matches.value_of("graveyard")
        .unwrap_or(GRAVEYARD));
    let sources: clap::Values = matches.values_of("SOURCE").unwrap();
    let cwd: PathBuf = current_dir().expect("Error getting current directory");
    if cwd.starts_with(graveyard) {
        println!("You should use rm to delete files in the graveyard.");
        return;
    }

    for source in sources {
        if let Err(e) = bury(source, &cwd, graveyard) {
            println!("ERROR: {}: {}", e, source);
        }
    }
}

fn bury(source: &str, cwd: &PathBuf, graveyard: &Path) -> std::io::Result<()> {

    let fullpath: PathBuf = cwd.join(Path::new(source));
    let dest: PathBuf = {
        // Can't join absolute paths, so we need to strip the leading "/"
        let grave = graveyard.join(fullpath.strip_prefix("/").unwrap());
        // Avoid a name conflict if necessary.
        if grave.exists() {
            // println!("found name conflict {}", grave.display());
            numbered_rename(&grave)
        } else {
            grave
        }
    };
    // println!("dest is {}", dest.display());


    // Try a simple rename, which will only work within the same mount point.
    // Trying to rename across filesystems will throw errno 18.
    if let Ok(_) = fs::rename(source, &dest) {
        return Ok(());
    }

    // If that didn't work, then copy and rm.
    if fullpath.is_dir() {
        fs::create_dir_all(&dest).expect("Failed to create grave path");
        for entry in WalkDir::new(source).into_iter().skip(1) {
            let entry = entry.expect("Failed to open file in source dir");
            let path: &Path = entry.path();
            let orphan: &Path = path.strip_prefix(source)
                .expect("Failed to descend into directory");
            if path.is_dir() {
                // println!("Creating {}", dest.join(orphan).display());
                try!(fs::create_dir(dest.join(orphan)));
            } else {
                // println!("Copying file {}", path.display());
                // println!("to {}", dest.join(orphan).display());
                try!(fs::copy(path, dest.join(orphan)));
            }
        }
        fs::remove_dir_all(source).expect("Failed to remove source dir");
    } else {
        fs::create_dir_all(dest.parent().unwrap())
            .expect("Failed to create grave path");
        try!(fs::copy(source, &dest));
        try!(fs::remove_file(source));
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
