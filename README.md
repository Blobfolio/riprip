# Rip Rip Hooray!

[![ci](https://img.shields.io/github/actions/workflow/status/Blobfolio/riprip/ci.yaml?style=flat-square&label=ci)](https://github.com/Blobfolio/riprip/actions)
[![deps.rs](https://deps.rs/repo/github/blobfolio/riprip/status.svg?style=flat-square&label=deps.rs)](https://deps.rs/repo/github/blobfolio/riprip)<br>
[![license](https://img.shields.io/badge/license-wtfpl-ff1493?style=flat-square)](https://en.wikipedia.org/wiki/WTFPL)
[![contributions welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg?style=flat-square&label=contributions)](https://github.com/Blobfolio/riprip/issues)


Rip Rip Hooray! is an iterative, [second-stage](#what-does-second-stage-mean) audio CD-ripper for Linux CLI, designed for advanced recovery/salvage of tracks from discs that have seen better days.

(It also provides a pretty summary of the disc details!)



## What Does Second-Stage Mean?

It means you should continue ripping your discs with _accurate_ CD-ripping software like [fre:ac](https://github.com/enzo1982/freac/) or [EAC](https://www.exactaudiocopy.de/) _first_, but if and when they fall short, switch over to Rip Rip Hooray to recover whatever problem tracks remain.

Those programs are nice and do lots of useful things, but their error recovery features are naive, ineffective, and scale _terribly_. You shouldn't waste your time with them. Burst or bust!

Rip Rip Hooray takes a gentler, iterative approach. Rather than beating a drive senseless for days on end, it simply collects what it can, as it can, progressively filling in the gaps across multiple passes.

Between runs you can clean the disc, switch drives, wait for better weather, see if you've already ripped enough for [CUETools repair](http://cue.tools/wiki/CUETools_Database) to get you across the finish line, etc.

Rip Rip Hooray does data recovery faster and better, plain and simple.

It will not, however, organize your media library, download album covers, convert shit between arbitrary formats, do the dishes, etc.

Hence "second-stage". ;)



## Features

Rip Rip Hooray's disc summary will give you:

* The basic table of contents in LSN format
* The corresponding `CDTOC` (for e.g. metadata)
* The [AccurateRip](http://www.accuraterip.com/), `CDDB`, [CUETools](http://cue.tools/wiki/CUETools_Database), and [MusicBrainz](https://musicbrainz.org/) disc IDs
* Any UPC/EAN and ISRC data stored on the disc

If that's all you want, there's a `--no-rip` flag you can pass to the program.

Rip-Rip-wise, it supports:

* [AccurateRip](http://www.accuraterip.com/), _et al_, drive-specific read offsets
* AccurateRip rip verification
* [CUETools](http://cue.tools/wiki/CUETools_Database) rip verification
* C2 Error Pointers
* Statefulness (you can pick up previous rips from where you left off)
* Individual track(s) or whole-disc ripping
* Automated or interactive pass limits
* Sample confirmation (to workaround faulty C2 data)
* Raw PCM and WAV output

Additional items on the roadmap:

* Drive offset auto-detection
* Ability to disable EAN/UPC/ISRC lookups
* Offline mode (disable AccurateRip/CTDB)



## Requirements

Rip Rip Hooray is a command line program for x86-64 Linux systems, though it might also work for other architectures and Unix-based operating systems.

It leverages `libcdio` for optical drive communications, so requires that system library to do its thing.

Pre-built `.deb` packages are attached to each [release](https://github.com/Blobfolio/riprip/releases) for Debian and Ubuntu users.

Everyone else can build it from source using `Rust`/`Cargo`:

```bash
# Clone the repository:
git clone https://github.com/Blobfolio/riprip

# The libcdio development headers are linked against, so must be present
# when building. On Debian/Ubuntu systems, this'll work:
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
