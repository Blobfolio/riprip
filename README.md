# Rip Rip Hooray!

[![ci](https://img.shields.io/github/actions/workflow/status/Blobfolio/riprip/ci.yaml?style=flat-square&label=ci)](https://github.com/Blobfolio/riprip/actions)
[![deps.rs](https://deps.rs/repo/github/blobfolio/riprip/status.svg?style=flat-square&label=deps.rs)](https://deps.rs/repo/github/blobfolio/riprip)<br>
[![license](https://img.shields.io/badge/license-wtfpl-ff1493?style=flat-square)](https://en.wikipedia.org/wiki/WTFPL)
[![contributions welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg?style=flat-square&label=contributions)](https://github.com/Blobfolio/riprip/issues)


Rip Rip Hooray! is a specialized audio CD-ripper optimized for track recovery.

It doesn't beat a drive senseless every time a read error is encountered; it simply notes the problem and moves on. Its iterative design allows it to grab what it can, as it can, progressively filling in the gaps from run-to-run.

Between those (relatively quick) runs, you can actually _do things_. You can inspect the disc, give it another clean, switch drives, shut down your computer and go to bed, or check to see the rip is already _good enough_ for [CUETools repair](http://cue.tools/wiki/CUETools_Database) to finish up for you.

Total recovery is not always possible, but Rip Rip Hooray! will rescue more data than traditional CD-ripping software, more accurately, and in significantly less time.



## Features

Iteration is key. Individual Rip Rip rips take minutes intead of hours or days, getting you access to the recovered data — regardless of "completeness" — as quickly as possible. You can re-run Rip Rip at any time, as many times as you want, with as many different optical drives as you want, to retry the outstanding regions and refine the data. You can also abort a rip early without losing your progress.

Beyond that, it supports all the good things:

* C2 error pointers
* Drive read offset auto-detection
* Drive read offset correction
* [AccurateRip](http://accuraterip.com/) checksum verification
* [CUETools](http://cue.tools/wiki/CUETools_Database) checksum verification
* Cache busting
* Sample re/confirmation
* Backwards ripping
* Raw PCM and WAV output

Rip Rip Hooray! does not aspire to manage your media library, so doesn't muck about with track metadata, format conversion, album art, etc. But it does print a nice little summary of the disc's table of contents and its various identifiers:

* [AccurateRip](http://accuraterip.com/) ID
* [CDDB](https://en.wikipedia.org/wiki/CDDB) ID
* [CUETools](http://cue.tools/wiki/CUETools_Database) ID
* [MusicBrainz](https://musicbrainz.org/) ID
* Track ISRCs (if present)
* UPC/EAN (if present)

That summary can be produced on its own using the `--no-rip` flag, if that's all you're interested in.



## Usage

Rip Rip Hooray! is run from the command line, like:

```bash
riprip [OPTIONS]

# To see a list of options, use -h/--help:
riprip --help
```

### Example Recovery Workflow

Start by ripping the entire disc using a traditional (and _accurate_!) CD ripper, like [fre:ac](https://github.com/enzo1982/freac/) or [Exact Audio Copy](https://www.exactaudiocopy.de/). Just be sure to:
* Rip each track to its own file
* Encode the tracks to a _lossless_ format, like FLAC or WAV
* Generate a cue sheet for the collection
* _Disable_ any error recovery-related options, or at least turn them way down

(Rip Rip _can_ be used to rip an entire disc, but the overhead associated with recovery would be overkill for the easy stuff.)

If any tracks are deemed "inaccurate", re-rip them with Rip Rip Hooray!:

```bash
# You might as well run this from the directory containing the original rip.
cd /path/to/traditional/rip

# Let's say tracks 4, 8, 9, and 10 are messed up.
riprip -t 4,8-10
```

It is possible one or more of the problem tracks will magically come out "accurate" on the first try, but in most cases they'll still be somewhat incomplete.

[CUETools](http://cue.tools/wiki/CUETools) has an amazing repair feature that can automatically fix up random sample errors. Before doing any further ripping, you should check to see if you've already ripped _enough_:
* Replace the bad tracks from the traditional rip with Rip Rip's versions, even if they're incomplete
* Open CUETools and point it to the cue sheet the traditional ripper generated for you
* Set the action to "Encode" and select "repair" from the dropdown
* Hit "Go" and cross your fingers!

If that worked, great! You're done!

If not, no worries. Just re-run Rip Rip to refine the data. If any of the tracks are in really rough shape, you can automate subsequent passes by using the `--refine` option:

```bash
# Same as before, but run through each track up to 5 times (1 + 4 extra).
riprip -t 4,8-10 --refine 4
```

Unless the damage is severe, Rip Rip and/or CUETools should eventually manage to turn the rip accurate!

### File I/O

Rip Rip Hooray! will need to create a number of different files in addition to the ripped tracks. To keep things tidy, it saves everything to its own subfolder within the current working directory called `_riprip`.

To resume a rip, just rerun the program from the same place, with the same disc, and it will automatically pick up from where it left off.

When you're completely done working on a disc — and have grabbed the exported tracks! — go ahead and delete the `_riprip` folder to reclaim the disk space. ;)

### Early Exit.

If you don't have time to let a rip finish naturally, press `CTRL+C` to stop it early. Your progress will still be saved, there just won't be as much of it. Haha.



## Installation

Debian and Ubuntu users can just grab the pre-built `.deb` package from the [release](https://github.com/Blobfolio/riprip/releases) page.

While specifically written for use on x86-64 Linux systems, both Rust and [libcdio](https://www.gnu.org/software/libcdio/) are cross-platform, so you may well be able to build it from source on other 64-bit Unix systems using `cargo`:

```bash
# Clone the repository:
git clone https://github.com/Blobfolio/riprip

# The libcdio development headers are required when building from source;
# Debian/Ubuntu users, for example, could run the following:
sudo apt-get install libcdio-dev

# Run Cargo build from the project root:
cd riprip
cargo build --release
```



## License

See also: [CREDITS.md](CREDITS.md)

Copyright © 2023 [Blobfolio, LLC](https://blobfolio.com) &lt;hello@blobfolio.com&gt;

This work is free. You can redistribute it and/or modify it under the terms of the Do What The Fuck You Want To Public License, Version 2.

    DO WHAT THE FUCK YOU WANT TO PUBLIC LICENSE
    Version 2, December 2004
    
    Copyright (C) 2004 Sam Hocevar <sam@hocevar.net>
    
    Everyone is permitted to copy and distribute verbatim or modified
    copies of this license document, and changing it is allowed as long
    as the name is changed.
    
    DO WHAT THE FUCK YOU WANT TO PUBLIC LICENSE
    TERMS AND CONDITIONS FOR COPYING, DISTRIBUTION AND MODIFICATION
    
    0. You just DO WHAT THE FUCK YOU WANT TO.
