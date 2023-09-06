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

That summary can be produced on its own using the `--no-rip` flag.



## Limitations and Workarounds

Rip Rip Hooray!, like any other CD-ripping tool, ultimately depends on the optical drive to correctly decode and deliver the requested data, or at least be accurate about the inaccuracies.

When a drive can't do that for whatever reason, the resulting rip will be incomplete or inaccurate.

The performance of Rip Rip Hooray! is similarly bound to that of the drive. It will always be magnitudes faster than `EAC`, _et al_, under the same conditions, but if the drive is struggling to make heads or tails of the disc, it might take a while to complete the rip.



## Usage

Rip Rip Hooray! is run from the command line, like:

```bash
riprip [FLAGS/OPTIONS]
```

### Ripping.

```text
    --no-c2       Disable/ignore C2 error pointer information when ripping,
                  e.g. for drives that do not support the feature. (This
                  flag is otherwise not recommended.)
    --no-cache-bust
                  Do not attempt to reset the optical drive cache between
                  each rip pass.
    --paranoia <NUM>
                  When a sample or its neighbors have a C2 or read error,
                  treat all samples in the region as supicious until the
                  drive returns the same value <NUM> times, or AccurateRip
                  or CTDB matches with a confidence of <NUM> are found.
                  When combined with --no-trust, *all* samples are subject
                  to confirmation regardless of status.
                  [default: 3; range: 1..=32]
    --raw         Save ripped tracks in raw PCM format (instead of WAV).
    --refine <NUM>
                  Execute up to <NUM> additional rip passes for each track
                  while any samples remain unread/unconfirmed.
                  [default: 0; max: 15]
-t, --track <NUM(s),RNG>
                  Rip one or more specific tracks (rather than the whole
                  disc). Multiple tracks can be separated by commas (2,3),
                  specified as an inclusive range (2-3), and/or given their
                  own -t/--track (-t 2 -t 3). [default: the whole disc]
```

### When All Else Fails…

```text
    --backwards   Rip sectors in reverse order. (Data will still be saved
                  in the *correct* order. Haha.)
    --no-resume   Ignore any previous rip states; start over from scratch.
    --no-trust    Never trust the drive when it says a sector is good;
                  always get confirmation. Requires a paranoia level of at
                  least 2.
    --reconfirm   Reset the status of all previously-accepted samples to
                  require reconfirmation. Requires a paranoia level of at
                  least 2.
```

### Drive Settings.

These options are auto-detected and do not usually need to be explicitly provided.

```text
-d, --dev <PATH>  The device path for the optical drive containing the CD
                  of interest, like /dev/cdrom.
-o, --offset <SAMPLES>
                  The AccurateRip, et al, sample read offset to apply to
                  data retrieved from the drive. [range: ±5880]
```

### Miscellaneous.

```text
-h, --help        Print help information and exit.
-V, --version     Print version information and exit.
    --no-rip      Just print the basic disc information to STDERR and exit.
```

### Early Exit.

If you don't have time to let a rip finish naturally, press `CTRL+C` to stop it early. Your progress will still be saved, there just won't be as much of it. Haha.

### Tracks, Logs, and State Data

Rip Rip Hooray! will need to create a number of different files in addition to the ripped tracks themselves. To keep things tidy, it saves everything to its own subfolder within the current working directory called `_riprip`.

To resume a rip, just rerun the program from the same place, with the same disc, and it will automatically pick up from where it left off.

When you're completely done working on a disc — and have moved or converted the exported tracks! — go ahead and delete the `_riprip` folder to reclaim the disk space. ;)



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
