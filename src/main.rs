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

    if matches.is_present("resurrect") {
        if let Some(s) = matches.values_of("resurrect") {
            for grave in s {
                let dest = grave.trim_left_matches(
                    graveyard.to_str().unwrap());
                if let Err(e) = bury(grave, dest) {
                    println!("ERROR: {}: {}", e, grave);
                } else {
                    println!("Returned {} to {}", grave, dest);
                }
            }
        } else if let Ok(s) = get_last_bury(record, graveyard) {
            let mut tokens = s.split("\t");
            tokens.next().expect("Bad record format: column A");
            let orig = tokens.next().expect("Bad record format: column B");
            let grave = tokens.next().expect("Bad record format: column C");
            let dest: &Path = &{
                if symlink_exists(orig) {
                    rename_grave(orig)
                } else {
                    PathBuf::from(orig)
                }
            };
            if let Err(e) = bury(grave, dest) {
                println!("ERROR: {}: {}", e, grave);
            } else if let Err(e) = write_log(grave, dest, record) {
                println!("Error adding {} to record: {}", grave, e);
            } else {
                println!("Returned {} to {}", grave, dest.display());
            }
        }
        return;
    }

    let cwd: PathBuf = env::current_dir().expect("Failed to get current dir");

    if matches.is_present("seance") {
        // Can't join absolute paths, so we need to strip the leading "/"
        let path = graveyard.join(cwd.strip_prefix("/").unwrap());
        let walkdir = if let Some(s) = matches.value_of("seance") {
            WalkDir::new(path).min_depth(1).max_depth(s.parse().unwrap())
        } else {
            WalkDir::new(path).min_depth(1)
        };
        for entry in walkdir.into_iter().filter_map(|e| e.ok()) {
            println!("{}", entry.path().display());
        }
        return;
    }

    if cwd.starts_with(graveyard) {
        // Not addressed: if you try to rip graveyard, it'll break very loudly
        println!("You should use rm to delete files in the graveyard, \
                  or --decompose to delete everything at once.");
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
                            .filter(|line| line.is_ok()) {
                            println!("> {}", line.unwrap());
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
    {
        let mut f = fs::OpenOptions::new()
                         .mode(0o666)
                         .create(true)
                         .append(true)
                         .open(record)?;
        f.write_all(format!("{}\t{}\t{}\n",
                            get_user(),
                            source.to_str().unwrap(),
                            dest.to_str().unwrap())
                    .as_bytes())?;
    }

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
fn get_last_bury<R, G>(record: R, graveyard: G) -> io::Result<String>
    where R: AsRef<Path>, G: AsRef<Path> {
    match fs::File::open(record) {
        Ok(mut f) => {
            let mut contents = String::new();
            f.read_to_string(&mut contents)?;
            let mut stack: Vec<&str> = Vec::new();

            for line in contents.lines().rev() {
                let mut tokens = line.split("\t");
                let user: &str = tokens.next().expect("Bad format: column A");
                let orig: &str = tokens.next().expect("Bad format: column B");
                let grave: &str = tokens.next().expect("Bad format: column C");

                // Only resurrect files buried by the same user
                if user != get_user() { continue }
                // Check if this is a resurrect.  If it is, then add the orig
                // file onto the stack to match with the last buried
                if Path::new(orig).starts_with(&graveyard) {
                    stack.push(orig);
                } else {
                    if let Some(p) = stack.pop() {
                        if p == grave { continue }
                    }
                    // If the top of the resurrect stack does not match the
                    // buried item, then this might be the file to bring back.
                    // Check that the file is still in the graveyard.
                    // If it is, return the corresponding line.
                    if symlink_exists(grave) {
                        return Ok(String::from(line))
                    } else {
                        // File was moved, remove the line from record

                    }
                }
            }
            Err(io::Error::new(io::ErrorKind::Other, "But nobody came"))
        },
        Err(e) => Err(e)
    }
}
