// -*- compile-command: "cargo build" -*-
#[macro_use]
extern crate clap;
extern crate core;
#[macro_use]
extern crate error_chain;
extern crate time;
extern crate walkdir;

use clap::{Arg, App};
use walkdir::WalkDir;
use std::{env, fs, io};
use std::path::{Path, PathBuf};
use std::io::{Read, Write, BufRead, BufReader};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::borrow::Cow;
mod errors {
    error_chain!{}
}
use errors::*;

include!("util.rs");

const GRAVEYARD: &'static str = "/tmp/graveyard";
const RECORD: &'static str = ".record";
const LINES_TO_INSPECT: usize = 6;
const FILES_TO_INSPECT: usize = 6;
const BIG_FILE_THRESHOLD: u64 = 500000000; // 500 MB

struct RecordItem<'a> {
    _time: &'a str,
    orig: &'a Path,
    dest: &'a Path,
}

fn main() {
    if let Err(ref e) = run() {
        let stderr = &mut ::std::io::stderr();
        let errmsg = "Error writing to stderr";

        writeln!(stderr, "error: {}", e).expect(errmsg);

        for e in e.iter().skip(1) {
            writeln!(stderr, "caused by: {}", e).expect(errmsg);
        }

        if let Some(backtrace) = e.backtrace() {
            writeln!(stderr, "backtrace: {:?}", backtrace).expect(errmsg);
        }

        ::std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let matches = App::new("rip")
        .version(crate_version!())
        .author(crate_authors!())
        .about("Rm ImProved
Send files to the graveyard (/tmp/graveyard-$USER by default) instead of unlinking them.")
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
            .help("Undo the last removal by the current user, or specify some file(s) in the \
                   graveyard.  Combine with -s to restore everything printed by -s.")
            .short("u")
            .long("unbury")
            .value_name("target")
            .min_values(0))
        .arg(Arg::with_name("inspect")
            .help("Prints some info about TARGET before prompting for action")
            .short("i")
            .long("inspect"))
        .get_matches();

    let _graveyard_opts = { 
        (matches.value_of("graveyard"),
         env::var("GRAVEYARD"),
         env::var("XDG_DATA_HOME"))
    };
    let _graveyard: Cow<str> = match _graveyard_opts {
        (Some(flag), _, _) => flag.into(),
        (_, Ok(env), _) => env.into(),
        (_, _, Ok(env)) => format!("{}{}", env, GRAVEYARD).into(),
        _ => format!("{}-{}", GRAVEYARD, get_user()).into(),
    };
    let graveyard = Path::new(&*_graveyard);

    if matches.is_present("decompose") {
        if prompt_yes("Really unlink the entire graveyard?") {
            fs::remove_dir_all(graveyard).chain_err(|| "Couldn't unlink graveyard")?;
        }
        return Ok(());
    }

    let record: &Path = &graveyard.join(RECORD);
    let cwd: PathBuf = env::current_dir().chain_err(|| "Failed to get current dir")?;

    if let Some(t) = matches.values_of("unbury") {
        // Vector to hold the grave path of items we want to unbury.
        // This will be used to determine which items to remove from the
        // record following the unbury.
        // Initialize it with the targets passed to -r
        let graves_to_exhume: &mut Vec<PathBuf> = &mut t.map(PathBuf::from).collect();

        // If -s is also passed, push all files found by seance onto
        // the graves_to_exhume.
        if matches.is_present("seance") {
            if let Ok(f) = fs::File::open(record) {
                let gravepath = join_absolute(graveyard, cwd)
                    .to_string_lossy()
                    .into_owned();
                for grave in seance(f, gravepath) {
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
        let f = fs::File::open(record).chain_err(|| "Couldn't read the record")?;
        for line in lines_of_graves(f, graves_to_exhume) {
            let entry: RecordItem = record_entry(&line);
            let orig: &Path = &{
                if symlink_exists(entry.orig) {
                    rename_grave(entry.orig)
                } else {
                    PathBuf::from(entry.orig)
                }
            };
            bury(entry.dest, orig).chain_err(|| {
                    format!("Unbury failed: couldn't copy files from {} to {}",
                            entry.dest.display(),
                            orig.display())
                })?;
            println!("Returned {} to {}", entry.dest.display(), orig.display());
        }

        // Reopen the record and then delete lines corresponding to exhumed graves
        if let Err(e) = fs::File::open(record)
            .and_then(|f| delete_lines_from_record(f, record, graves_to_exhume)) {
            bail!("Failed to remove unburied files from record: {}", e);
        }
        return Ok(());
    }

    if matches.is_present("seance") {
        let gravepath = join_absolute(graveyard, cwd);
        let f = fs::File::open(record).chain_err(|| "Failed to read record")?;
        for grave in seance(f, gravepath.to_string_lossy()) {
            println!("{}", grave.display());
        }
        return Ok(());
    }

    if let Some(targets) = matches.values_of("TARGET") {
        for target in targets {
            // Check if source exists
            if let Ok(metadata) = fs::symlink_metadata(target) {
                // Canonicalize the path unless it's a symlink
                let source = &if !metadata.file_type().is_symlink() {
                    cwd.join(target).canonicalize().chain_err(|| "Failed to canonicalize path")?
                } else {
                    cwd.join(target)
                };

                if matches.is_present("inspect") {
                    if metadata.is_dir() {
                        // Get the size of the directory and all its contents
                        println!("{}: directory, {} including:",
                                 target,
                                 humanize_bytes(WalkDir::new(source)
                                     .into_iter()
                                     .filter_map(|x| x.ok())
                                     .filter_map(|x| x.metadata().ok())
                                     .map(|x| x.len())
                                     .sum::<u64>()));

                        // Print the first few top-level files in the directory
                        for entry in WalkDir::new(source)
                            .min_depth(1)
                            .max_depth(1)
                            .into_iter()
                            .filter_map(|entry| entry.ok())
                            .take(FILES_TO_INSPECT) {
                            println!("{}", entry.path().display());
                        }
                    } else {
                        println!("{}: file, {}", target, humanize_bytes(metadata.len()));
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
                    if !prompt_yes(format!("Send {} to the graveyard?", target)) {
                        continue;
                    }
                }

                // If rip is called on a file already in the graveyard, prompt
                // to permanently delete it instead.
                if source.starts_with(graveyard) {
                    println!("{} is already in the graveyard.", source.display());
                    if prompt_yes("Permanently unlink it?") {
                        if fs::remove_dir_all(source).is_err() {
                            fs::remove_file(source).chain_err(|| "Couldn't unlink")?;
                        }
                        continue;
                    } else {
                        println!("Skipping {}", source.display());
                        return Ok(());
                    }
                }

                let dest: &Path = &{
                    let dest = join_absolute(graveyard, source);
                    // Resolve a name conflict if necessary
                    if symlink_exists(&dest) {
                        rename_grave(dest)
                    } else {
                        dest
                    }
                };

                bury(source, dest).or_else(|e| {
                        fs::remove_dir_all(dest).is_ok();
                        Err(e)
                    })
                    .chain_err(|| "Failed to bury file")?;
                // Clean up any partial buries due to permission error
                write_log(source, dest, record)
                    .chain_err(|| format!("Failed to write record at {}", record.display()))?;
            } else {
                bail!("Cannot remove {}: no such file or directory", target);
            }
        }
    } else {
        println!("{}\nrip -h for help", matches.usage());
    }

    Ok(())
}

/// Write deletion history to record
fn write_log<S, D, R>(source: S, dest: D, record: R) -> io::Result<()>
    where S: AsRef<Path>,
          D: AsRef<Path>,
          R: AsRef<Path>
{
    let (source, dest) = (source.as_ref(), dest.as_ref());
    let mut f = fs::OpenOptions::new().create(true)
        .append(true)
        .open(record)?;
    writeln!(f,
             "{}\t{}\t{}",
             time::now().ctime(),
             source.display(),
             dest.display())?;

    Ok(())
}

fn bury<S: AsRef<Path>, D: AsRef<Path>>(source: S, dest: D) -> Result<()> {
    let (source, dest) = (source.as_ref(), dest.as_ref());
    // Try a simple rename, which will only work within the same mount point.
    // Trying to rename across filesystems will throw errno 18.
    if fs::rename(source, dest).is_ok() {
        return Ok(());
    }

    // If that didn't work, then copy and rm.
    let parent = dest.parent().ok_or("Couldn't get parent of dest")?;
    fs::create_dir_all(parent).chain_err(|| "Couldn't create parent dir")?;

    if fs::symlink_metadata(source).chain_err(|| "Couldn't get metadata")?.is_dir() {
        // Walk the source, creating directories and copying files as needed
        for entry in WalkDir::new(source).into_iter().filter_map(|e| e.ok()) {
            // Path without the top-level directory
            let orphan: &Path = entry.path()
                .strip_prefix(source)
                .chain_err(|| "Parent directory isn't a prefix of child directories?")?;
            if entry.file_type().is_dir() {
                fs::create_dir_all(dest.join(orphan)).chain_err(|| {
                        format!("Failed to create {} in {}",
                                entry.path().display(),
                                dest.join(orphan).display())
                    })?;
            } else {
                copy_file(entry.path(), dest.join(orphan)).chain_err(|| {
                        format!("Failed to copy file from {} to {}",
                                entry.path().display(),
                                dest.join(orphan).display())
                    })?;
            }
        }
        fs::remove_dir_all(source)
            .chain_err(|| format!("Failed to remove dir: {}", source.display()))?;
    } else {
        copy_file(source, dest).chain_err(|| {
                format!("Failed to copy file from {} to {}",
                        source.display(),
                        dest.display())
            })?;
        fs::remove_file(source)
            .chain_err(|| format!("Failed to remove file: {}", source.display()))?;
    }

    Ok(())
}

fn copy_file<S: AsRef<Path>, D: AsRef<Path>>(source: S, dest: D) -> io::Result<()> {
    let (source, dest) = (source.as_ref(), dest.as_ref());
    let metadata = fs::symlink_metadata(source)?;
    let filetype = metadata.file_type();

    if metadata.len() > BIG_FILE_THRESHOLD {
        println!("About to copy a big file ({} is {})",
                 source.display(),
                 humanize_bytes(metadata.len()));
        if prompt_yes("Permanently delete this file instead?") {
            return Ok(());
        }
    }

    if filetype.is_file() {
        if let Err(e) = fs::copy(source, dest) {
            // println!("Failed to copy {} to {}", source.display(), dest.display());
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
    } else if let Err(e) = fs::copy(source, dest) {
        // Special file: Try copying it as normal, but this probably won't work
        println!("Non-regular file or directory: {}", source.display());
        if !prompt_yes("Permanently delete the file?") {
            return Err(e);
        }
        // Create a dummy file to act as a marker in the graveyard
        let mut marker = fs::File::create(dest)?;
        marker.write_all(b"This is a marker for a file that was \
                           permanently deleted.  Requiescat in pace.")?;
    }

    Ok(())
}

/// Return the path in the graveyard of the last file to be buried.
/// As a side effect, any valid last files that are found in the record but
/// not on the filesystem are removed from the record.
fn get_last_bury<R: AsRef<Path>>(record: R) -> io::Result<PathBuf> {
    let graves_to_exhume: &mut Vec<PathBuf> = &mut Vec::new();
    let mut f = fs::File::open(record.as_ref())?;
    let mut contents = String::new();
    f.read_to_string(&mut contents)?;

    // This could be cleaned up more if/when for loops can return a value
    for entry in contents.lines().rev().map(record_entry) {
        // Check that the file is still in the graveyard.
        // If it is, return the corresponding line.
        if symlink_exists(entry.dest) {
            if !graves_to_exhume.is_empty() {
                delete_lines_from_record(f, record, graves_to_exhume)?;
            }
            return Ok(PathBuf::from(entry.dest));
        } else {
            // File is gone, mark the grave to be removed from the record
            graves_to_exhume.push(PathBuf::from(entry.dest));
        }
    }

    if !graves_to_exhume.is_empty() {
        delete_lines_from_record(f, record, graves_to_exhume)?;
    }
    Err(io::Error::new(io::ErrorKind::NotFound, "But nobody came"))
}

/// Parse a line in the record into a `RecordItem`
fn record_entry(line: &str) -> RecordItem {
    let mut tokens = line.split('\t');
    let time: &str = tokens.next().expect("Bad format: column A");
    let orig: &str = tokens.next().expect("Bad format: column B");
    let dest: &str = tokens.next().expect("Bad format: column C");
    RecordItem {
        _time: time,
        orig: Path::new(orig),
        dest: Path::new(dest),
    }
}

/// Takes a vector of grave paths and returns the respective lines in the record
fn lines_of_graves<'a>(f: fs::File, graves: &'a [PathBuf]) -> impl Iterator<Item = String> + 'a {
    BufReader::new(f)
        .lines()
        .filter_map(|l| l.ok())
        .filter(move |l| graves.into_iter().any(|y| y == record_entry(l).dest))
}

/// Returns an iterator over all graves in the record that are under gravepath
fn seance<T: AsRef<str>>(f: fs::File, gravepath: T) -> impl Iterator<Item = PathBuf> {
    BufReader::new(f)
        .lines()
        .filter_map(|l| l.ok())
        .map(|l| PathBuf::from(record_entry(&l).dest))
        .filter(move |d| d.starts_with(gravepath.as_ref()))
}

/// Takes a vector of grave paths and removes the respective lines from the record
fn delete_lines_from_record<R: AsRef<Path>>(f: fs::File,
                                            record: R,
                                            graves: &[PathBuf])
                                            -> io::Result<()> {
    let record = record.as_ref();
    // Get the lines to write back to the record, which is every line except
    // the ones matching the exhumed graves.  Store them in a vector
    // since we'll be overwriting the record in-place.
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
