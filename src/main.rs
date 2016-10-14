// -*- compile-command: "cargo build" -*-
#![feature(io)]
#![feature(alloc_system)]
#![feature(core_str_ext)]
extern crate alloc_system;
#[macro_use]
extern crate clap;
extern crate core;
extern crate walkdir;
extern crate libc;

use clap::{Arg, App};
use core::str::StrExt;
use walkdir::WalkDir;
use std::path::{Path, PathBuf};
use std::fs;
use std::env;
use std::io;
use std::io::{Read, Write, BufRead, BufReader};
use std::process::Command;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::DirBuilderExt;
use libc::umask;

static GRAVEYARD: &'static str = "/tmp/.graveyard";
static HISTFILE: &'static str = ".rip_history";
fn main() {
    let matches = App::with_defaults("rip")
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
        if prompt_yes("Really unlink the entire graveyard?"){
            fs::remove_dir_all(graveyard).is_ok();
        }
        return;
    }

    if matches.is_present("resurrect") {
        let histfile: &Path = &graveyard.join(HISTFILE);
        if let Ok(s) = get_last_bury(histfile, graveyard) {
            let mut tokens = StrExt::split(s.as_str(), "\t");
            let _ = tokens.next().expect("Bad histfile format: column A");
            let orig = tokens.next().expect("Bad histfile format: column B");
            let grave = tokens.next().expect("Bad histfile format: column C");
            let source = Path::new(grave);
            let dest = Path::new(orig);
            if let Err(e) = bury(source, dest) {
                println!("ERROR: {}: {}", e, source.display());
                println!("Maybe the file was removed from the graveyard.");
                // if prompt_yes("Remove it from the history?") {
                //     delete_last_line(histfile).unwrap();
                // }
            } else if let Err(e) = write_log(source, dest, graveyard) {
                println!("Error adding {} to histfile: {}",
                         source.display(), e);
            } else {
                println!("Returned {} to {}",
                         source.display(), dest.display());
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
        // Not addressed: if you try to rip graveyard, it'll break very loudly
        println!("You should use rm to delete files in the graveyard, \
                  or --decompose to delete everything at once.");
        return;
    }

    unsafe {
        umask(0);
    }

    if let Some(targets) = matches.values_of("TARGET") {
        for target in targets {
            let path: PathBuf = cwd.join(Path::new(target));
            if !path.exists() {
                println!("Cannot remove {}: no such file or directory",
                         path.display());
                return;
            }
            // Can't join absolute paths, so we need to strip the leading "/"
            let dest: PathBuf = {
                let grave = graveyard.join(path.strip_prefix("/").unwrap());
                if grave.exists() { rename_grave(grave) } else { grave }
            };
            if let Err(e) = bury(&path, &dest) {
                println!("ERROR: {}: {}", e, target);
            } else if let Err(e) = write_log(&path, &dest, graveyard) {
                println!("Error adding {} to histfile: {}", target, e);
            }
        }
    } else {
        println!("{}\nrip -h for help", matches.usage());
    }
}

/// Write deletion history to HISTFILE
fn write_log(source: &Path, dest: &Path, graveyard: &Path)
             -> io::Result<()> {
    let histfile = graveyard.join(HISTFILE);
    {
        let mut f = try!(fs::OpenOptions::new()
                         .mode(0o666)
                         .create(true)
                         .append(true)
                         .open(histfile));
        try!(f.write_all(
            format!("{}\t{}\t{}\n",
                    get_user(),
                    source.to_str().unwrap(),
                    dest.to_str().unwrap(),
            ).as_bytes()));
    }

    Ok(())
}

fn bury(source: &Path, dest: &Path) -> io::Result<()> {
    // Try a simple rename, which will only work within the same mount point.
    // Trying to rename across filesystems will throw errno 18.
    if let Ok(_) = fs::rename(source, dest) {
        return Ok(());
    }

    // If that didn't work, then copy and rm.
    if try!(fs::symlink_metadata(source)).is_dir() {
        // Walk the source, creating directories and copying files as needed
        for entry in WalkDir::new(source).into_iter() {
            let entry = try!(entry);
            let path: &Path = entry.path();
            // Path without the top-level directory
            let orphan: &Path = path.strip_prefix(source).unwrap();

            if path.is_dir() {
                let dir = fs::DirBuilder::new()
                    .mode(0o777)
                    .recursive(true)
                    .create(dest.join(orphan));
                if let Err(e) = dir {
                    println!("Failed to create {} in {}",
                             path.display(),
                             dest.join(orphan).display());
                    try!(fs::remove_dir_all(dest));
                    return Err(e);
                }
            } else {
                try!(copy_file(path, dest.join(orphan).as_path()));
            }
        }
        try!(fs::remove_dir_all(source));
    } else {
        let parent = dest.parent().expect("A file without a parent?");
        try!(fs::DirBuilder::new().mode(0o777).recursive(true).create(parent));
        try!(copy_file(source, dest));
        try!(fs::remove_file(source));
    }

    Ok(())
}

fn copy_file(source: &Path, dest: &Path) -> io::Result<()> {
    let filetype = try!(fs::symlink_metadata(source)).file_type();
    if filetype.is_file() {
        if let Err(e) = fs::copy(source, dest) {
            println!("Failed to copy {} to {}",
                     source.display(), dest.display());
            return Err(e);
        }
    } else if filetype.is_fifo() {
        try!(Command::new("mkfifo").arg(dest).output());
    } else if filetype.is_symlink() {
        let target = try!(fs::read_link(source));
        try!(std::os::unix::fs::symlink(target, dest));
    } else {
        // Special file: Try copying it as normal, but this probably won't work
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

// fn warn_big_file(filedata: fs::Metadata) -> bool {
//     let threshold = 500000000;
//     if filedata.size() > threshold {
//         println!("About to copy a big file ({} bytes}", filedata.len());
//         return prompt_yes("Permanently delete this file instead?")
//     }
// }

/// return a WalkDir iterator that excludes the top-level directory.
fn walk_into_dir<P: AsRef<Path>>(path: P) -> std::iter::Skip<walkdir::Iter> {
    WalkDir::new(path).into_iter().skip(1)
}

/// Prompt for user input, returning True if the first character is 'y' or 'Y'
fn prompt_yes(prompt: &str) -> bool {
    print!("{} (y/n) ", prompt);
    io::stdout().flush().unwrap();
    let stdin = io::stdin();
    if let Some(c) = stdin.lock().chars().next() {
        if let Ok(c) = c {
            return c == 'y' || c == 'Y';
        }
    }
    false
}

/// Return the line in histfile corresponding to the last buried file still in
/// the graveyard
fn get_last_bury(path: &Path, graveyard: &Path) -> io::Result<String> {
    match fs::File::open(path) {
        Ok(f) => {
            let lines: Vec<String> = BufReader::new(f)
                .lines()
                .map(|line| line.unwrap())
                .collect();
            let mut stack: Vec<&str> = Vec::new();

            for line in lines.iter().rev() {
                let mut tokens = StrExt::split(line.as_str(), "\t");
                let user: &str = tokens.next().expect("Bad format: column A");
                let orig: &str = tokens.next().expect("Bad format: column B");
                let grave: &str = tokens.next().expect("Bad format: column C");

                // Only resurrect files buried by the same user
                if user != get_user() { continue }
                // Check if this is a resurrect.  If it is, then add the orig
                // file onto the stack to match with the last buried
                if Path::new(orig).starts_with(graveyard) {
                    stack.push(orig);
                } else {
                    if let Some(p) = stack.pop() {
                        if p == grave { continue }
                    }
                    // If the top of the resurrect stack does not match the
                    // buried item, then assume it's still in the graveyard
                    return Ok(line.clone())
                }
            }
            return Err(io::Error::new(io::ErrorKind::Other, "But nobody came"))
        },
        Err(e) => Err(e)
    }
}

fn get_user() -> String {
    env::var("USER").unwrap_or(String::from("unknown"))
}
