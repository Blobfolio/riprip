# Rip Rip Hooray!

[![ci](https://img.shields.io/github/actions/workflow/status/Blobfolio/riprip/ci.yaml?style=flat-square&label=ci)](https://github.com/Blobfolio/riprip/actions)
[![deps.rs](https://deps.rs/repo/github/blobfolio/riprip/status.svg?style=flat-square&label=deps.rs)](https://deps.rs/repo/github/blobfolio/riprip)<br>
[![license](https://img.shields.io/badge/license-wtfpl-ff1493?style=flat-square)](https://en.wikipedia.org/wiki/WTFPL)
[![contributions welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg?style=flat-square&label=contributions)](https://github.com/Blobfolio/riprip/issues)


Rip Rip Hooray! is a specialized audio CD-ripper optimized for track recovery.

It doesn't beat a drive senseless every time a read error is encountered; it simply notes the problem and moves on. Its iterative design allows it to grab what it can, as it can, progressively filling in the gaps from run-to-run.

Between those runs — which typically only last a few minutes — you can actually _do things_. You can inspect the disc, give it another clean, switch drives, shut down your computer and go to bed, or check to see the rip is already _good enough_ for [CUETools repair](http://cue.tools/wiki/CUETools_Database) to automatically finish.

Total recovery is not always possible, but Rip Rip Hooray! will rescue more data than traditional CD-ripping software, more accurately, and in significantly less time.



## Features

Iteration is key. Individual Rip Rip rips take minutes intead of hours or days, getting you access to the recovered data — regardless of "completeness" — as quickly as possible. You can re-run Rip Rip at any time, as many times as you want, with as many different optical drives as you want, to retry the outstanding regions and refine the data.

Beyond that, it supports all the good things:

* C2 error pointers
* Drive read offset auto-detection
* Drive read offset correction
* [AccurateRip](http://accuraterip.com/) checksum verification
* [CUETools](http://cue.tools/wiki/CUETools_Database) checksum verification
* Cache busting
* Raw PCM and WAV output
* Sample re/confirmation

Rip Rip Hooray! does not aspire to manage your media library, so doesn't muck about with track metadata, format conversion, album art, etc. But it does print a nice little summary of the disc's table of contents and its various identifiers:

* [AccurateRip](http://accuraterip.com/) ID
* [CDDB](https://en.wikipedia.org/wiki/CDDB) ID
* [CUETools](http://cue.tools/wiki/CUETools_Database) ID
* [MusicBrainz](https://musicbrainz.org/) ID
* Track ISRCs (if present)
* UPC/EAN (if present)



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
