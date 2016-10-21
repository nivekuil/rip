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
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::fs::PermissionsExt;
include!("util.rs");

static GRAVEYARD: &'static str = "/tmp/.graveyard";
static RECORD: &'static str = ".record";
const LINES_TO_INSPECT: usize = 6;

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
Send files to the graveyard (/tmp/.graveyard) instead of unlinking them.")
        .arg(Arg::with_name("TARGET")
             .help("File or directory to remove")
             .multiple(true)
             .index(1))
        .arg(Arg::with_name("graveyard")
             .help("Directory where deleted files go to rest")
             .long("graveyard")
             .takes_value(true))
        .arg(Arg::with_name("decompose")
             .help("Permanently delete (unlink) the entire graveyard")
             .short("d")
             .long("decompose"))
        .arg(Arg::with_name("seance")
             .help("List all objects in graveyard that were sent from the \
                    current directory, or specify a depth.")
             .short("s")
             .long("seance")
             .value_name("depth")
             .min_values(0))
        .arg(Arg::with_name("resurrect")
             .help("Undo the last removal by the current user, or specify \
                    some file(s) in the graveyard.")
             .short("r")
             .long("resurrect")
             .value_name("target")
             .min_values(0))
        .arg(Arg::with_name("inspect")
             .help("Print some info about TARGET before prompting for action")
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
    // Disable umask so rip can create a globally writable graveyard
    unsafe {
        libc::umask(0);
    }

    if let Some(mut s) = matches.values_of("resurrect") {
        // Vector to hold the grave path of resurrected items that we want
        // to remove from the record following the resurrect
        let mut graves_to_exhume: Vec<String> = Vec::new();
        // Handle any arguments were passed to --resurrect
        while let Some(grave) = s.next() {
            let dest = grave.trim_left_matches(
                graveyard.to_str().unwrap());
            if let Err(e) = bury(grave, dest) {
                println!("ERROR: {}: {}", e, grave);
            } else {
                println!("Returned {} to {}", grave, dest);
            }
            graves_to_exhume.push(grave.to_string());
        }

        // Otherwise, return the last deleted file
        if graves_to_exhume.len() == 0 {
            if let Ok(s) = get_last_bury(record) {
                let (orig, grave) = {
                    let record_line = record_line(s.as_str());
                    (record_line.orig, record_line.dest)
                };
                let dest: &Path = &{
                    if symlink_exists(orig) {
                        rename_grave(orig)
                    } else {
                        PathBuf::from(orig)
                    }
                };
                if let Err(e) = bury(grave, dest) {
                    println!("ERROR: {}: {}", e, grave);
                }  else {
                    println!("Returned {} to {}", grave, dest.display());
                }
                graves_to_exhume.push(grave.to_string());
            }
        }

        if let Err(e) = delete_line_from_record(record, graves_to_exhume) {
            println!("Failed to delete resurrects from grave record: {}", e)
        };

        return;
    }

    let cwd: PathBuf = env::current_dir().expect("Failed to get current dir");

    if matches.is_present("seance") {
        // Can't join absolute paths, so we need to strip the leading "/"
        let path = graveyard.join(cwd.strip_prefix("/").unwrap());
        if let Ok(f) = fs::File::open(record) {
            for line in BufReader::new(f).lines().filter_map(|l| l.ok()) {
                let dest = record_line(line.as_str()).dest;
                if dest.starts_with(path.to_str().unwrap()) {
                    println!("{}", dest);
                }
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
                        println!("{}: directory, {} objects", target,
                                 WalkDir::new(source).into_iter().count());
                    } else {
                        println!("{}: file, {} bytes", target, metadata.len());
                        // Read the file and print the first 6 lines
                        let f = fs::File::open(source).unwrap();
                        for line in BufReader::new(f)
                            .lines()
                            .take(LINES_TO_INSPECT)
                            .filter_map(|line| line.ok()) {
                                println!("> {}", line);
                            }
                    }
                    if !prompt_yes(&format!("Send {} to the graveyard?",
                                            target)) {
                        continue;
                    }
                }
            } else {
                println!("Cannot remove {}: no such file or directory",
                         target);
                return;
            }

            // If rip is called on a file already in the graveyard, prompt
            // to permanently delete it instead.
            if source.starts_with(graveyard) {
                println!("{} is already in the graveyard.", source.display());
                if prompt_yes("Permanently unlink it?") {
                    if let Err(_) = fs::remove_dir_all(source) {
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

    if filetype.is_file() {
        if let Err(e) = fs::copy(source, dest) {
            println!("Failed to copy {} to {}",
                     source.display(), dest.display());
            return Err(e);
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
    } else {
        // Special file: Try copying it as normal, but this probably won't work
        if let Err(e) = fs::copy(source, dest) {
            println!("Non-regular file or directory: {}", source.display());
            if !prompt_yes("Permanently delete the file?") {
                return Err(e);
            }
            // Create a dummy file to act as a marker in the graveyard
            let mut marker = fs::File::create(dest)?;
            marker.write_all(b"This is a marker for a file that was \
                               permanently deleted.  Requiescat in pace.")?;
        }
    }

    Ok(())
}

// fn warn_big_file(filedata: fs::Metadata) -> bool {
//     let threshold = 500000000;
//     if filedata.size() > threshold {
//         println!("About to copy a big file ({} bytes}", filedata.len());
//         return prompt_yes("Permanently delete this file instead?")
//     }
// }

/// Return the line in record corresponding to the last buried file still in
/// the graveyard
fn get_last_bury<R: AsRef<Path>>(record: R) -> io::Result<String> {
    let record = record.as_ref();
    match fs::File::open(record) {
        Ok(mut f) => {
            let mut contents = String::new();
            f.read_to_string(&mut contents)?;

            for line in contents.lines().rev() {
                let (user, grave) = {
                    let record_line = record_line(line);
                    (record_line.user, record_line.dest)
                };
                // Only resurrect files buried by the same user
                if user != get_user() { continue }

                // Check that the file is still in the graveyard.
                // If it is, return the corresponding line.
                if symlink_exists(grave) {
                    return Ok(String::from(line))
                } else {
                    // File was moved, remove the line from record
                    delete_line_from_record(record, vec![String::from(line)])?;
                }
            }
            Err(io::Error::new(io::ErrorKind::Other, "But nobody came"))
        },
        Err(e) => Err(e)
    }
}

/// Parse a line in the record into a RecordItem
fn record_line(line: &str) -> RecordItem {
    let mut tokens = line.split("\t");
    let user: &str = tokens.next().expect("Bad format: column A");
    let orig: &str = tokens.next().expect("Bad format: column B");
    let dest: &str = tokens.next().expect("Bad format: column C");
    RecordItem { user: user, orig: orig, dest: dest }
}

fn delete_line_from_record<R>(record: R, graves_to_exhume: Vec<String>)
                              -> io::Result<()> where R: AsRef<Path> {
    let record = record.as_ref();
    // Get the lines to write back to record
    let lines: Vec<String> = {
        match fs::File::open(record) {
            Ok(f) => BufReader::new(f)
                .lines()
                .filter_map(|l| l.ok())
                .filter(|l| graves_to_exhume.clone().into_iter()
                        .any(|y| y != record_line(l.as_str()).dest))
                .collect(),
            Err(e) => return Err(e)
        }
    };
    match fs::File::create(record) {
        Ok(mut f) => for line in lines {
            writeln!(f, "{}", line).unwrap();
        },
        Err(e) => return Err(e)
    }
    Ok(())
}
