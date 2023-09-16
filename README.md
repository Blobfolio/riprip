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
* Subchannel timecode synchronization
* Drive read offset auto-detection
* Drive read offset correction
* [AccurateRip](http://accuraterip.com/) checksum verification
* [CUETools](http://cue.tools/wiki/CUETools_Database) checksum verification
* HTOA (can rip the pre-gap track, if any)
* Cache busting
* Sample re/confirmation
* Backwards ripping
* Raw PCM and WAV output
* CUE sheet generation (when ripping whole disc in WAV format)

Rip Rip Hooray! does not aspire to manage your media library, so doesn't muck about with track metadata, format conversion, album art, etc. But it does print a nice little summary of the disc's table of contents and its various identifiers:

* [AccurateRip](http://accuraterip.com/) ID
* [CDDB](https://en.wikipedia.org/wiki/CDDB) ID
* [CUETools](http://cue.tools/wiki/CUETools_Database) ID
* [MusicBrainz](https://musicbrainz.org/) ID
* Track ISRCs (if present)
* UPC/EAN (if present)

That summary can be produced on its own using the `--no-rip` flag, if that's all you're interested in.



## Limitations

Like any CD-ripper, Rip Rip Hooray! is ultimately dependent on the optical drive to correctly read and report the data from the disc, or at least be accurate about any inaccuracies it passes down.

If a drive isn't up to the task, the resulting rip may be incomplete or inaccurate.

Unlike traditional CD-rippers, Rip Rip Hooray! needs to store _a lot_ of detailed state information for each track in order to mitigate drive inconsistencies and keep track of which sectors need (re)reading, which ones don't, etc.

This information is only needed while it's needed — you can delete the `_riprip` subfolder as soon as you've gotten what you wanted — but is nonetheless hefty, generally about 1-3x the size of the original CD source.

The peak memory usage of Rip Rip Hooray! is comparable to some traditional CD-ripping software, albeit for completely different reasons. Depending on the size of the track, the consistency of its data, and how the system allocates resources, it could reach 1-3 GiB, maybe a little more.

Recovery isn't free, but it's damn satisfying. Haha.



## Usage

Rip Rip Hooray! is run from the command line, like:

```bash
riprip [OPTIONS]

# To see a list of options, use -h/--help:
riprip --help
```

### Example Recovery Workflow

First things first, rip the entire disc and see what happens!

With no arguments, Rip Rip Hooray! will simply rip the entire disc, including the HTOA (if any). Each track will be saved to its own WAV file, and a cue sheet will be generated for the collection.

```bash
# Rip everything, one pass per track.
riprip
```

Audio CD rips cannot be _directly_ verified, but they can be _statistically_ verified. Rip Rip automatically checks each track after each pass to see if there are sufficient matches in the AccurateRip and CUETools databases. If there are, it will call the rip **GOOD** and move on.

If there are any non-confirmed (problem) tracks remaining after the first pass, open the cue sheet with [CUETools](http://cue.tools/wiki/CUETools) to see if automatic repair is possible. If it is, great! CUETools will fix everything up and give you a perfect copy of each track. (You can also use CUETools to fill in metadata, convert formats, etc.)

If problems remain, don't worry; _iterate_!

Simply re-run Rip Rip. It will pick up from where it left off, (re)reading any sectors that have room for improvement (and skipping the rest).

```bash
# Refine the original rip.
riprip

# If you plan to re-rip several times, you can automate the process with the
# -p/--passes option:
riprip --passes 3
```

As before, if problem tracks remain after the re-rip, open the cue sheet in CUETools, etc. 

Rinse and repeat until all tracks have been confirmed, or your drive has read everything it possibly can.

Good luck!



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
