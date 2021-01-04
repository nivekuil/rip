* rip (Rm ImProved)
[[https://crates.io/crates/rm-improved][https://img.shields.io/crates/v/rm-improved.svg]]
[[https://travis-ci.org/nivekuil/rip][https://travis-ci.org/nivekuil/rip.svg?branch=master]]

=rip= is a command-line deletion tool focused on safety, ergonomics, and performance.  It favors a simple interface, and does /not/ implement the xdg-trash spec or attempt to achieve the same goals.

Deleted files get sent to the graveyard (=/tmp/graveyard-$USER= by default, see [[https://github.com/nivekuil/rip#-notes][notes]] on changing this) under their absolute path, giving you a chance to recover them.  No data is overwritten.  If files that share the same path are deleted, they will be renamed as numbered backups.

=rip= is made for lazy people.  If any part of the interface could be more intuitive, please open an issue or pull request.

** ⚰ Installation
   Or get a binary [[https://github.com/nivekuil/rip/releases][release]] (Linux x86_64, ARMv7 and macOS), untar it, and move it somewhere on your $PATH:
   #+BEGIN_EXAMPLE
   $ tar xvzf rip-*.tar.gz
   $ mv rip /usr/local/bin
   #+END_EXAMPLE

   or build it:
   #+BEGIN_EXAMPLE
   $ cargo install rm-improved
   #+END_EXAMPLE

   Arch Linux users can install it from the [[https://aur.archlinux.org/packages/rm-improved/][AUR]] (thanks @Charon77!)
   #+BEGIN_EXAMPLE
   $ yay -S rm-improved
   #+END_EXAMPLE
   
   macOS users can install it with Homebrew:

   #+BEGIN_EXAMPLE
   $ brew install rm-improved
   #+END_EXAMPLE

** ⚰ Usage
   #+BEGIN_EXAMPLE
   USAGE:
       rip [FLAGS] [OPTIONS] [TARGET]...

   FLAGS:
       -d, --decompose    Permanently deletes (unlink) the entire graveyard
       -h, --help         Prints help information
       -i, --inspect      Prints some info about TARGET before prompting for action
       -s, --seance       Prints files that were sent under the current directory
       -V, --version      Prints version information

   OPTIONS:
           --graveyard <graveyard>    Directory where deleted files go to rest
       -u, --unbury <target>       Undo the last removal by the current user, or specify some file(s) in the graveyard.  Combine with -s to restore everything printed by -s.

   ARGS:
       <TARGET>...    File or directory to remove
   #+END_EXAMPLE
   Basic usage -- easier than rm
   #+BEGIN_EXAMPLE
   $ rip dir1/ file1
   #+END_EXAMPLE
   Undo the last deletion
   #+BEGIN_EXAMPLE
   $ rip -u
   Returned /tmp/graveyard-jack/home/jack/file1 to /home/jack/file1
   #+END_EXAMPLE
   Print some info (size and first few lines in a file, total size and first few files in a directory) about the target and then prompt for deletion
   #+BEGIN_EXAMPLE
   $ rip -i file1
   dir1: file, 1337 bytes including:
   > Position: Shooting Guard and Small Forward ▪ Shoots: Right
   > 6-6, 185lb (198cm, 83kg)
   Send file1 to the graveyard? (y/n) y
   #+END_EXAMPLE
   Print files that were deleted from under the current directory
   #+BEGIN_EXAMPLE
   $ rip -s
   /tmp/graveyard-jack/home/jack/file1
   /tmp/graveyard-jack/home/jack/dir1
   #+END_EXAMPLE
   Name conflicts are resolved
   #+BEGIN_EXAMPLE
   $ touch file1
   $ rip file1
   $ rip -s
   /tmp/graveyard-jack/home/jack/dir1
   /tmp/graveyard-jack/home/jack/file1
   /tmp/graveyard-jack/home/jack/file1~1
   #+END_EXAMPLE
   -u also takes the path of a file in the graveyard
   #+BEGIN_EXAMPLE
   $ rip -u /tmp/graveyard-jack/home/jack/file1
   Returned /tmp/graveyard-jack/home/jack/file1 to /home/jack/file1
   #+END_EXAMPLE
   Combine -u and -s to restore everything printed by -s
   #+BEGIN_EXAMPLE
   $ rip -su
   Returned /tmp/graveyard-jack/home/jack/dir1 to /home/jack/dir1
   Returned /tmp/graveyard-jack/home/jack/file1~1 to /home/jack/file1~1
   #+END_EXAMPLE
*** Emacs
    #+begin_src emacs-lisp
      (setq delete-by-moving-to-trash t)
      (defun system-move-file-to-trash (filename)
        (shell-command (concat (executable-find "rip") " " filename)))
    #+end_src

** ⚰ Notes
   - You probably shouldn't alias =rm= to =rip=.  Unlearning muscle memory is hard, but it's harder to ensure that every =rm= you make (as different users, from different machines and application environments) is the aliased one.
   - If you have =$XDG_DATA_HOME= environment variable set, =rip= will use =$XDG_DATA_HOME/graveyard= instead of the =/tmp/graveyard-$USER=.
   - If you want to put the graveyard somewhere else (like =~/.local/share/Trash=), you have two options, in order of precedence:
     1. Alias =rip= to =rip --graveyard ~/.local/share/Trash=
     2. Set the environment variable =$GRAVEYARD= to =~/.local/share/Trash=.
     This can be a good idea because if the graveyard is mounted on an in-memory filesystem (as /tmp is in Arch Linux), deleting large files can quickly fill up your RAM.  It's also much slower to move files across filesystems, although the delay should be minimal with an SSD.
   - In general, a deletion followed by a =--unbury= should be idempotent.
   - The deletion log is kept in =.record=, found in the top level of the graveyard.
