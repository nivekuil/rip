// -*- compile-command: "cargo build" -*-
#![feature(io)]
#![feature(alloc_system)]
extern crate alloc_system;
#[macro_use]
extern crate clap;
extern crate core;
extern crate walkdir;
extern crate libc;

use clap::{Arg, App};
use walkdir::WalkDir;
use std::path::{Path, PathBuf};
use std::fs;
use std::env;
use std::io;
use std::io::{Read, Write, BufRead, BufReader};
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, DirBuilderExt, PermissionsExt};
include!("util.rs");

const GRAVEYARD: &'static str = "/tmp/.graveyard";
const RECORD: &'static str = ".record";
const LINES_TO_INSPECT: usize = 6;
const FILES_TO_INSPECT: usize = 6;
const BIG_FILE_THRESHOLD: u64 = 500000000; // 500 MB

struct RecordItem<'a> {
    user: &'a str,
    orig: &'a str,
    dest: &'a str,
}

fn main() {
    let matches = App::new("rip")
        .version(crate_version!())
        .author(crate_authors!())
        .about("Rm ImProved
Send files to the graveyard (/tmp/.graveyard by default) instead of unlinking them.")
        .arg(Arg::with_name("TARGET")
             .help("File or directory to remove")
             .multiple(true)
             .index(1))
        .arg(Arg::with_name("graveyard")
             .help("Directory where deleted files go to rest")
             .long("graveyard")
             .takes_value(true))
        .arg(Arg::with_name("decompose")
             .help("Permanently deletes (unlink) the entire graveyard")
             .short("d")
             .long("decompose"))
        .arg(Arg::with_name("seance")
             .help("Prints files that were sent under the current directory")
             .short("s")
             .long("seance"))
        .arg(Arg::with_name("unbury")
             .help("Undo the last removal by the current user, or specify \
                    some file(s) in the graveyard.  Combine with -s to \
                    restore everything printed by -s.")
             .short("u")
             .long("unbury")
             .value_name("target")
             .min_values(0))
        .arg(Arg::with_name("inspect")
             .help("Prints some info about TARGET before prompting for action")
             .short("i")
             .long("inspect"))
        .get_matches();

    let graveyard: &Path = &PathBuf::from(
        match (matches.value_of("graveyard"), env::var("GRAVEYARD")) {
            (Some(flag), _) => flag.to_string(),
            (_, Ok(env)) => env,
            _ => GRAVEYARD.to_string(),
        }
    );

    if matches.is_present("decompose") {
        if prompt_yes("Really unlink the entire graveyard?"){
            if let Err(e) = fs::remove_dir_all(graveyard) {
                println!("ERROR: {}", e);
            }
        }
        return;
    }

    let record: &Path = &graveyard.join(RECORD);
    let cwd: PathBuf = env::current_dir().expect("Failed to get current dir");
    // Disable umask so rip can create a globally writable graveyard
    unsafe {
        libc::umask(0);
    }

    if let Some(t) = matches.values_of("unbury") {
        // Vector to hold the grave path of items we want to unbury.
        // This will be used to determine which items to remove from the
        // record following the unbury.
        // Initialize it with the targets passed to -r
        let mut graves_to_exhume: Vec<String> = t.map(String::from).collect();

        // If -s is also passed, push all files found by seance onto
        // the graves_to_exhume.
        if matches.is_present("seance") {
            if let Ok(f) = fs::File::open(record) {
                let gravepath = graveyard.join(cwd.strip_prefix("/").unwrap());
                let seanced = BufReader::new(f)
                    .lines()
                    .filter_map(|l| l.ok())
                    .map(|l| record_entry(&l).dest.to_string())
                    .filter(|d| d.starts_with(gravepath.to_str().unwrap()));
                for grave in seanced {
                    graves_to_exhume.push(grave);
                }
            }
        }

        // Otherwise, add the last deleted file
        if graves_to_exhume.is_empty() {
            if let Ok(s) = get_last_bury(record) {
                graves_to_exhume.push(s);
            }
        }

        // Go through the graveyard and exhume all the graves
        let f = &fs::File::open(record).unwrap();
        for line in lines_of_graves(f, &graves_to_exhume) {
            let entry = record_entry(&line);
            let orig: &Path = &{
                if symlink_exists(entry.orig) {
                    rename_grave(entry.orig)
                } else {
                    PathBuf::from(entry.orig)
                }
            };
            if let Err(e) = bury(entry.dest, orig) {
                println!("ERROR: {}: {}", e, entry.dest);
            } else {
                println!("Returned {} to {}", entry.dest, orig.display());
            }
        }
        // Go through the record and remove all the exhumed graves
        if let Err(e) = delete_lines_from_record(f, record, &graves_to_exhume) {
            println!("Failed to delete unburys from grave record: {}", e)
        };
        return;
    }

    if matches.is_present("seance") {
        // Can't join absolute paths, so we need to strip the leading "/"
        let path = graveyard.join(cwd.strip_prefix("/").unwrap());
        if let Ok(f) = fs::File::open(record) {
            for line in BufReader::new(f)
                .lines()
                .filter_map(|l| l.ok())
                .map(|l| record_entry(&l).dest.to_string())
                .filter(|l| l.starts_with(path.to_str().unwrap())) {
                    println!("{}", line);
                }
        }
        return;
    }

    if let Some(targets) = matches.values_of("TARGET") {
        for target in targets {
            let source: &Path = &cwd.join(Path::new(target));

            // Check if source exists
            if let Ok(metadata) = source.symlink_metadata() {

                if matches.is_present("inspect") {
                    if metadata.is_dir() {
                        // Get the size of the directory and all its contents
                        println!("{}: directory, {} including:", target,
                                  humanize_bytes(
                                      WalkDir::new(source)
                                          .into_iter()
                                          .filter_map(|x| x.ok())
                                          .filter_map(|x| x.metadata().ok())
                                          .map(|x| x.len())
                                          .sum::<u64>()));
                                 
                        // Print the first few top-level files in the directory
                        for entry in WalkDir::new(source)
                            .min_depth(1).max_depth(1).into_iter()
                            .filter_map(|entry| entry.ok())
                            .take(FILES_TO_INSPECT) {
                                println!("{}", entry.path().display());
                            }
                    } else {
                        println!("{}: file, {}", target,
                                 humanize_bytes(metadata.len()));
                        // Read the file and print the first few lines
                        if let Ok(f) = fs::File::open(source) {
                            for line in BufReader::new(f)
                                .lines()
                                .take(LINES_TO_INSPECT)
                                .filter_map(|line| line.ok()) {
                                    println!("> {}", line);
                                }
                        } else {
                            println!("Error reading {}", source.display());
                        }
                    }
                    if !prompt_yes(&format!("Send {} to the graveyard?", target)) {
                        continue;
                    }
                }

            } else {
                println!("Cannot remove {}: no such file or directory", target);
                return;
            }

            // If rip is called on a file already in the graveyard, prompt
            // to permanently delete it instead.
            if source.starts_with(graveyard) {
                println!("{} is already in the graveyard.", source.display());
                if prompt_yes("Permanently unlink it?") {
                    if fs::remove_dir_all(source).is_err() {
                        if let Err(e) = fs::remove_file(source) {
                            println!("Couldn't unlink {}:", e);
                        }
                    }
                    continue;
                }
            }

            let dest: &Path = &{
                // Can't join absolute paths, so strip the leading "/"
                let dest = graveyard.join(source.strip_prefix("/").unwrap());
                // Resolve a name conflict if necessary
                if symlink_exists(&dest) {
                    rename_grave(dest)
                } else {
                    dest
                }
            };

            if let Err(e) = bury(source, dest) {
                println!("ERROR: {}: {}", e, target);
                // Clean up any partial buries due to permission error
                fs::remove_dir_all(dest).is_ok();
            } else if let Err(e) = write_log(source, dest, record) {
                println!("Error adding {} to record: {}", target, e);
            }
        }
    } else {
        println!("{}\nrip -h for help", matches.usage());
    }
}

/// Write deletion history to record
fn write_log<S, D, R>(source: S, dest: D, record: R) -> io::Result<()>
    where S: AsRef<Path>, D: AsRef<Path>, R: AsRef<Path> {
    let (source, dest) = (source.as_ref(), dest.as_ref());
    let mut f = fs::OpenOptions::new()
        .mode(0o666)
        .create(true)
        .append(true)
        .open(record)?;
    writeln!(f, "{}\t{}\t{}", get_user(), source.display(), dest.display())?;

    Ok(())
}

fn bury<S, D>(source: S, dest: D) -> io::Result<()>
    where S: AsRef<Path>, D: AsRef<Path> {
    let (source, dest) = (source.as_ref(), dest.as_ref());
    // Try a simple rename, which will only work within the same mount point.
    // Trying to rename across filesystems will throw errno 18.
    if fs::rename(source, dest).is_ok() {
        return Ok(());
    }

    // If that didn't work, then copy and rm.
    // All parent directories are created with open permissions so that
    // other users can delete things too
    let parent = dest.parent().expect("Trying to delete root?");
    fs::DirBuilder::new().mode(0o777).recursive(true).create(parent)?;

    if fs::symlink_metadata(source)?.is_dir() {
        // Walk the source, creating directories and copying files as needed
        for entry in WalkDir::new(source).into_iter().filter_map(|e| e.ok()) {
            // Path without the top-level directory
            let orphan: &Path = entry.path().strip_prefix(source).unwrap();
            if entry.file_type().is_dir() {
                let mode = entry.metadata()?.permissions().mode();
                if let Err(e) = fs::DirBuilder::new()
                    .mode(mode)
                    .create(dest.join(orphan)) {
                    println!("Failed to create {} in {}",
                             entry.path().display(),
                             dest.join(orphan).display());
                    fs::remove_dir_all(dest)?;
                    return Err(e);
                }
            } else {
                copy_file(entry.path(), dest.join(orphan))?;
            }
        }
        fs::remove_dir_all(source)?;
    } else {
        copy_file(source, dest)?;
        fs::remove_file(source)?;
    }

    Ok(())
}

fn copy_file<S, D>(source: S, dest: D) -> io::Result<()>
    where S: AsRef<Path>, D: AsRef<Path> {
    let (source, dest) = (source.as_ref(), dest.as_ref());
    let metadata = fs::symlink_metadata(source)?;
    let filetype = metadata.file_type();

    if metadata.len() > BIG_FILE_THRESHOLD {
        println!("About to copy a big file ({} is {})", source.display(),
                 humanize_bytes(metadata.len()));
        if prompt_yes("Permanently delete this file instead?") {
            return Ok(())
        }
    }

    if filetype.is_file() {
        if let Err(e) = fs::copy(source, dest) {
            println!("Failed to copy {} to {}",
                     source.display(), dest.display());
            return Err(e)
        }
    } else if filetype.is_fifo() {
        let mode = metadata.permissions().mode();
        std::process::Command::new("mkfifo")
            .arg(dest)
            .arg("-m")
            .arg(mode.to_string());
    } else if filetype.is_symlink() {
        let target = fs::read_link(source)?;
        std::os::unix::fs::symlink(target, dest)?;
    } else if let Err(e) = fs::copy(source, dest) {
        // Special file: Try copying it as normal, but this probably won't work
        println!("Non-regular file or directory: {}", source.display());
        if !prompt_yes("Permanently delete the file?") {
            return Err(e)
        }
        // Create a dummy file to act as a marker in the graveyard
        let mut marker = fs::File::create(dest)?;
        marker.write_all(b"This is a marker for a file that was \
                           permanently deleted.  Requiescat in pace.")?;
    }

    Ok(())
}

/// Return the path in the graveyard of the last file buried by the user
fn get_last_bury<R: AsRef<Path>>(record: R) -> io::Result<String> {
    let record = record.as_ref();
    let mut graves_to_exhume: &mut Vec<String> = &mut Vec::new();
    let mut f = fs::File::open(record)?;
    let mut contents = String::new();
    f.read_to_string(&mut contents)?;

    // This could be cleaned up more if/when for loops can return a value
    for entry in contents.lines().rev()
        .map(record_entry)
        .filter(|x| x.user == get_user()) {
            // Check that the file is still in the graveyard.
            // If it is, return the corresponding line.
            if symlink_exists(entry.dest) {
                if !graves_to_exhume.is_empty() {
                    delete_lines_from_record(&f, record, &graves_to_exhume)?;
                }
                return Ok(String::from(entry.dest))
            } else {
                // File is gone, mark the grave to be removed from the record
                graves_to_exhume.push(String::from(entry.dest));
            }
        }

    if !graves_to_exhume.is_empty() {
        delete_lines_from_record(&f, record, &graves_to_exhume)?;
    }
    Err(io::Error::new(io::ErrorKind::Other, "But nobody came"))
}

/// Parse a line in the record into a `RecordItem`
fn record_entry(line: &str) -> RecordItem {
    let mut tokens = line.split('\t');
    let user: &str = tokens.next().expect("Bad format: column A");
    let orig: &str = tokens.next().expect("Bad format: column B");
    let dest: &str = tokens.next().expect("Bad format: column C");
    RecordItem { user: user, orig: orig, dest: dest }
}

/// Takes a vector of grave paths and returns the respective lines in the record
fn lines_of_graves(f: &fs::File, graves: &[String]) -> Vec<String> {
    BufReader::new(f)
        .lines()
        .filter_map(|l| l.ok())
        .filter(|l| graves.into_iter().any(|y| y == record_entry(l).dest))
        .collect()
}

/// Takes a vector of grave paths and removes the respective lines from the record
fn delete_lines_from_record<R: AsRef<Path>>(f: &fs::File,
                                            record: R,
                                            graves: &[String])
                                            -> io::Result<()> {
    let record = record.as_ref();
    // Get the lines to write back to the record, which is every line except
    // the ones matching the exhumed graves.  Store them in a vector
    // since we'll be overwriting the record in-place
    let lines_to_write: Vec<String> = BufReader::new(f)
        .lines()
        .filter_map(|l| l.ok())
        .filter(|l| !graves.into_iter().any(|y| y == record_entry(l).dest))
        .collect();
    let mut f = fs::File::create(record)?;
    for line in lines_to_write {
        writeln!(f, "{}", line)?;
    }

    Ok(())
}
