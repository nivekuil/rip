// -*- compile-command: "cargo build" -*-
#![feature(io)]
#![feature(alloc_system)]
#![feature(core_str_ext)]
extern crate alloc_system;
#[macro_use]
extern crate clap;
extern crate core;
extern crate walkdir;

use clap::{Arg, App};
use core::str::StrExt;
use walkdir::WalkDir;
use std::path::{Path, PathBuf};
use std::fs;
use std::env;
use std::io::{Read, Write, BufRead, BufReader};

static GRAVEYARD: &'static str = "/tmp/.graveyard";
static HISTFILE: &'static str = ".rip_history";

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
             .conflicts_with("seance")
             .conflicts_with("resurrect"))
        .arg(Arg::with_name("graveyard")
             .help("Directory where deleted files go to rest")
             .long("graveyard")
             .takes_value(true))
        .arg(Arg::with_name("decompose")
             .help("Permanently delete (unlink) the entire graveyard")
             .long("decompose"))
        .arg(Arg::with_name("seance")
             .help("List all objects in the graveyard that were sent from the \
                    current directory")
             .short("s")
             .long("seance"))
        .arg(Arg::with_name("resurrect")
             .help("Undo the last removal")
             .short("r")
             .long("resurrect"))
        .get_matches();

    let graveyard: &Path = &PathBuf::from(
        match (matches.value_of("graveyard"), env::var("GRAVEYARD")) {
            (Some(flag), _) => flag.to_string(),
            (_, Ok(env)) => env,
            _ => GRAVEYARD.to_string(),
        }
    );

    if matches.is_present("decompose") {
        fs::remove_dir_all(graveyard).is_ok();
        return;
    }

    if matches.is_present("resurrect") {
        let histfile: PathBuf = graveyard.join(HISTFILE);
        if let Ok(s) = read_last_line(&histfile) {
            let mut tokens = StrExt::split(s.as_str(), "\t");
            let dest = tokens.next().expect("Bad histfile format: column A");
            let source = tokens.next().expect("Bad histfile format: column B");
            if let Err(e) = bury(Path::new(source), Path::new(dest)) {
                println!("ERROR: {}: {}", e, source);
                println!("Maybe the file was removed from the graveyard.");
                if prompt_yes("Remove it from the history?") {
                    delete_last_line(&histfile).unwrap();
                }

            } else {
                println!("Returned {} to {}", source, dest);
                delete_last_line(&histfile).expect("Failed to remove history");
            }
        }
        return;
    }

    let cwd: PathBuf = env::current_dir().expect("Failed to get current dir");

    if matches.is_present("seance") {
        // Can't join absolute paths, so we need to strip the leading "/"
        let path = graveyard.join(cwd.strip_prefix("/").unwrap());
        for entry in walk_into_dir(path) {
            println!("{}", entry.unwrap().path().display());
        }
        return;
    }

    if cwd.starts_with(graveyard) {
        println!("You should use rm to delete files in the graveyard, \
                  or --decompose to delete everything at once.");
        return;
    }

    for source in matches.values_of("SOURCE").unwrap() {
        let path: PathBuf = cwd.join(Path::new(source));
        if !path.exists() {
            println!("Cannot remove {}: no such file or directory",
                     path.display());
            return;
        }
        let dest: PathBuf = {
            // Can't join absolute paths, so we need to strip the leading "/"
            let grave = graveyard.join(path.strip_prefix("/").unwrap());
            if grave.exists() { rename_grave(grave) } else { grave }
        };
        if let Err(e) = bury(&path, &dest) {
            println!("ERROR: {}: {}", e, source);
        } else if let Err(e) = write_log(&path, &dest, graveyard) {
            println!("Error adding {} to histfile: {}", source, e);
        }
    }
}

/// Write deletion history to HISTFILE in the format "SOURCEPATH\tGRAVEPATH\n".
fn write_log(source: &PathBuf, dest: &PathBuf, graveyard: &Path)
             -> std::io::Result<()> {
    let histfile = graveyard.join(HISTFILE);
    {
        let mut f = try!(fs::OpenOptions::new()
                         .create(true)
                         .append(true)
                         .open(histfile));
        try!(f.write_all(format!("{}\t{}\n",
                                 source.to_str().unwrap(),
                                 dest.to_str().unwrap())
                         .as_bytes()));
    }

    Ok(())
}

fn bury(source: &Path, dest: &Path) -> std::io::Result<()> {
    // Try a simple rename, which will only work within the same mount point.
    // Trying to rename across filesystems will throw errno 18.
    if let Ok(_) = fs::rename(source, dest) {
        return Ok(());
    }

    // If that didn't work, then copy and rm.
    let filedata = try!(fs::metadata(source));
    if filedata.is_dir() {
        // Create all directories including the top-level dir, and then
        // skip the top-level dir in WalkDir because it may be renamed
        // due to name collision
        fs::create_dir_all(dest).expect("Failed to create grave path");
        // Walk the source, creating directories and copying files as needed
        for entry in walk_into_dir(source) {
            let entry = try!(entry);
            let path: &Path = entry.path();
            // Path without the top-level directory
            let orphan: &Path = path.strip_prefix(source).unwrap();
            if path.is_dir() {
                if let Err(e) = fs::create_dir(dest.join(orphan)) {
                    println!("Failed to create {} in {}",
                             path.display(),
                             dest.join(orphan).display());
                    try!(fs::remove_dir_all(dest));
                    return Err(e);
                }
            } else {
                if let Err(e) = fs::copy(path, dest.join(orphan)) {
                    println!("Failed to copy {} to {}",
                             path.display(),
                             dest.join(orphan).display());
                    try!(fs::remove_dir_all(dest));
                    return Err(e);
                }
            }
        }
        try!(fs::remove_dir_all(source));
    } else if filedata.is_file() {
        let parent = dest.parent().unwrap();
        fs::create_dir_all(parent).expect("Failed to create grave path");
        if let Err(e) = fs::copy(source, dest) {
            println!("Failed to copy {} to {}",
                     source.display(), dest.display());
            return Err(e);
        }
        if let Err(e) = fs::remove_file(source) {
            println!("Failed to remove {}", source.display());
            return Err(e);
        }
    } else {
        // Special file: Try copying it as normal, but this probably won't work
        let parent = dest.parent().unwrap();
        fs::create_dir_all(parent).expect("Failed to create grave path");
        if let Err(e) = fs::copy(source, dest) {
            println!("Non-regular file or directory: {}", source.display());
            if !prompt_yes("Permanently delete the file?") {
                return Err(e);
            }
            // Create a dummy file to act as a marker in the graveyard
            let mut marker = try!(fs::File::create(dest));
            try!(marker.write_all(b"This is a marker for a file that was per\
                                    manently deleted.  Requiescat in pace."));
        }
        try!(fs::remove_file(source));
    }

    Ok(())
}

/// Add a numbered extension to duplicate filenames to avoid overwriting files.
fn rename_grave(grave: PathBuf) -> PathBuf {
    if grave.extension().is_none() {
        (1_u64..)
            .map(|i| grave.with_extension(i.to_string()))
            .skip_while(|p| p.exists())
            .next()
            .expect("Failed to rename duplicate file or directory")
    } else {
        (1_u64..)
            .map(|i| {
                grave.with_extension(format!("{}.{}",
                                             grave.extension()
                                             .unwrap()
                                             .to_str()
                                             .unwrap(),
                                             i))
            })
            .skip_while(|p| p.exists())
            .next()
            .expect("Failed to rename duplicate file or directory")
    }
}

/// Return a WalkDir iterator that excludes the top-level directory.
fn walk_into_dir<P: AsRef<Path>>(path: P) -> std::iter::Skip<walkdir::Iter> {
    WalkDir::new(path).into_iter().skip(1)
}

/// Prompt for user input, returning True if the first character is 'y' or 'Y'
fn prompt_yes(prompt: &str) -> bool {
    print!("{} (y/n) ", prompt);
    std::io::stdout().flush().unwrap();
    let stdin = std::io::stdin();
    if let Some(c) = stdin.lock().chars().next() {
        if let Ok(c) = c {
            return c == 'y' || c == 'Y';
        }
    }
    false
}

fn read_last_line(path: &PathBuf) -> std::io::Result<String> {
    match fs::File::open(path) {
        Ok(f) => BufReader::new(f).lines().last().expect("Empty histfile"),
        Err(e) => Err(e)
    }
}

/// Set the length of the file to the difference between the size of the file
/// and the size of last line of the file.
fn delete_last_line(path: &PathBuf) -> std::io::Result<()> {
    match fs::OpenOptions::new().write(true).open(path) {
        Ok(f) => {
            let total: u64 = f.metadata().expect("Failed to stat file").len();
            let last_line: usize = try!(read_last_line(path)).bytes().count();
            let difference = total - last_line as u64 - 1;
            // Remove histfile if it would be truncated to 0 to avoid a panic
            if difference == 0 {
                try!(fs::remove_file(path));
            } else {
                f.set_len(difference).expect("Failed to truncate file");
            }

            Ok(())
        },
        Err(e) => Err(e)
    }
}
