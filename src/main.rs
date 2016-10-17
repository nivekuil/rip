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

static GRAVEYARD: &'static str = "/tmp/.graveyard";
static HISTFILE: &'static str = ".rip_history";

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
             .help("List all objects in the graveyard that were sent from the \
                    current directory")
             .short("s")
             .long("seance"))
        .arg(Arg::with_name("resurrect")
             .help("Undo the last removal by the current user")
             .short("r")
             .long("resurrect"))
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
            fs::remove_dir_all(graveyard).is_ok();
        }
        return;
    }

    let histfile: &Path = &graveyard.join(HISTFILE);
    // Disable umask so rip can create a globally writable graveyard
    unsafe {
        libc::umask(0);
    }

    if matches.is_present("resurrect") {
        if let Ok(s) = get_last_bury(histfile, graveyard) {
            let mut tokens = s.split("\t");
            tokens.next().expect("Bad histfile format: column A");
            let orig = tokens.next().expect("Bad histfile format: column B");
            let grave = tokens.next().expect("Bad histfile format: column C");
            let dest: &Path = &{
                if symlink_exists(orig) {
                    rename_grave(orig)
                } else {
                    PathBuf::from(orig)
                }
            };
            if let Err(e) = bury(grave, dest) {
                println!("ERROR: {}: {}", e, grave);
            } else if let Err(e) = write_log(grave, dest, histfile) {
                println!("Error adding {} to histfile: {}", grave, e);
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
        for entry in WalkDir::new(path).min_depth(1) {
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

    if let Some(targets) = matches.values_of("TARGET") {
        for target in targets {
            let path: &Path = &cwd.join(Path::new(target));

            // Check if path exists
            if let Ok(metadata) = path.symlink_metadata() {
                if matches.is_present("inspect") {
                    if metadata.is_dir() {
                        println!("{}: directory, {} objects", target,
                                 WalkDir::new(target).into_iter().count());
                    } else {
                        println!("{}: file, {} bytes", target, metadata.len());
                        // Read the file and print the first 6 lines
                        let f = fs::File::open(target).unwrap();
                        for line in BufReader::new(f).lines().take(6) {
                            println!("> {}", line.unwrap());
                        }
                    }
                    if !prompt_yes(&format!("Send {} to the graveyard?", target)) {
                        continue;
                    }
                }
            } else {
                println!("Cannot remove {}: no such file or directory",
                         path.display());
                return;
            }

            let dest: &Path = &{
                // Can't join absolute paths, so strip the leading "/"
                let dest = graveyard.join(path.strip_prefix("/").unwrap());
                // Resolve a name conflict if necessary
                if symlink_exists(&dest) {
                    rename_grave(dest)
                } else {
                    dest
                }
            };

            if let Err(e) = bury(path, dest) {
                println!("ERROR: {}: {}", e, target);
            } else if let Err(e) = write_log(path, dest, histfile) {
                println!("Error adding {} to histfile: {}", target, e);
            }
        }
    } else {
        println!("{}\nrip -h for help", matches.usage());
    }
}

/// Write deletion history to histfile
fn write_log<S, D, H>(source: S, dest: D, histfile: H) -> io::Result<()>
    where S: AsRef<Path>, D: AsRef<Path>, H: AsRef<Path> {
    let (source, dest) = (source.as_ref(), dest.as_ref());
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
    try!(fs::DirBuilder::new().mode(0o777).recursive(true).create(parent));
    if try!(fs::symlink_metadata(source)).is_dir() {
        // Walk the source, creating directories and copying files as needed
        for entry in WalkDir::new(source).into_iter().filter_map(|e| e.ok()) {
            // Path without the top-level directory
            let orphan: &Path = entry.path().strip_prefix(source).unwrap();
            if entry.file_type().is_dir() {
                let mode = try!(entry.metadata()).permissions().mode();
                if let Err(e) = fs::DirBuilder::new()
                    .mode(mode)
                    .create(dest.join(orphan)) {
                    println!("Failed to create {} in {}",
                             entry.path().display(),
                             dest.join(orphan).display());
                    try!(fs::remove_dir_all(dest));
                    return Err(e);
                }
            } else {
                try!(copy_file(entry.path(), dest.join(orphan)));
            }
        }
        try!(fs::remove_dir_all(source));
    } else {
        try!(copy_file(source, dest));
        try!(fs::remove_file(source));
    }

    Ok(())
}

fn copy_file<S, D>(source: S, dest: D) -> io::Result<()>
    where S: AsRef<Path>, D: AsRef<Path> {
    let (source, dest) = (source.as_ref(), dest.as_ref());
    let metadata = try!(fs::symlink_metadata(source));
    let filetype = metadata.file_type();
    if filetype.is_file() {
        if let Err(e) = fs::copy(source, dest) {
            println!("Failed to copy {} to {}",
                     source.display(), dest.display());
            return Err(e);
        }
    } else if filetype.is_fifo() {
        let path = std::ffi::CString::new(dest.to_str().unwrap()).unwrap();
        let mode = metadata.permissions().mode();
        unsafe {
            libc::mkfifo(path.as_ptr(), mode);
        }
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
fn rename_grave<G: AsRef<Path>>(grave: G) -> PathBuf {
    let grave = grave.as_ref();
    if grave.extension().is_none() {
        (1_u64..)
            .map(|i| grave.with_extension(i.to_string()))
            .skip_while(|p| symlink_exists(p))
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
            .skip_while(|p| symlink_exists(p))
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

/// Prompt for user input, returning True if the first character is 'y' or 'Y'
fn prompt_yes(prompt: &str) -> bool {
    print!("{} (y/n) ", prompt);
    io::stdout().flush().unwrap();
    let stdin = BufReader::new(io::stdin());
    if let Some(c) = stdin.chars().next() {
        if let Ok(c) = c {
            return c == 'y' || c == 'Y';
        }
    }
    false
}

/// Return the line in histfile corresponding to the last buried file still in
/// the graveyard
fn get_last_bury<H, G>(histfile: H, graveyard: G) -> io::Result<String>
    where H: AsRef<Path>, G: AsRef<Path> {
    match fs::File::open(histfile) {
        Ok(f) => {
            let lines: Vec<String> = BufReader::new(f)
                .lines()
                .map(|line| line.unwrap())
                .collect();
            let mut stack: Vec<&str> = Vec::new();

            for line in lines.iter().rev() {
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
                    if symlink_exists(grave) {
                        return Ok(line.clone())
                    }
                }
            }
            return Err(io::Error::new(io::ErrorKind::Other, "But nobody came"))
        },
        Err(e) => Err(e)
    }
}

fn symlink_exists<P: AsRef<Path>>(path: P) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn get_user() -> String {
    env::var("USER").unwrap_or(String::from("unknown"))
}
