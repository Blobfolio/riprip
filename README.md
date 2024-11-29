# Rip Rip Hooray!

[![ci](https://img.shields.io/github/actions/workflow/status/Blobfolio/riprip/ci.yaml?style=flat-square&label=ci)](https://github.com/Blobfolio/riprip/actions)
[![deps.rs](https://deps.rs/repo/github/blobfolio/riprip/status.svg?style=flat-square&label=deps.rs)](https://deps.rs/repo/github/blobfolio/riprip)<br>
[![license](https://img.shields.io/badge/license-wtfpl-ff1493?style=flat-square)](https://en.wikipedia.org/wiki/WTFPL)
[![contributions welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg?style=flat-square&label=contributions)](https://github.com/Blobfolio/riprip/issues)


Rip Rip Hooray! is a specialized audio CD-ripper optimized for track recovery.

It doesn't beat a drive senseless every time a read error is encountered; it simply notes the problem and moves on. Its iterative design allows it to grab what it can, as it can, progressively filling in the gaps from run-to-run.

Between those (relatively quick) runs, you can actually _do things_. You can inspect the disc, give it another clean, switch drives, shut down your computer and go to bed, or check to see the rip is already _good enough_ for [CUETools repair](http://cue.tools/wiki/CUETools_Database) to finish up for you.

Total recovery is not always possible — drives can only read what they can read — but Rip Rip Hooray! will rescue more data than traditional CD-ripping software, more accurately, and in significantly less time.



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
* Good ol' WAV output
* Cue sheet generation (when ripping the whole disc)

Rip Rip Hooray! **does not** aspire to manage your media library, so doesn't muck about with track metadata, format conversion, album art, etc.

But it does print a nice little summary of the disc's table of contents and its various TOC-derived and encoded identifiers:

* [AccurateRip](http://accuraterip.com/) ID
* [CDDB](https://en.wikipedia.org/wiki/CDDB) ID
* [CUETools](http://cue.tools/wiki/CUETools_Database) ID
* [MusicBrainz](https://musicbrainz.org/) ID
* Track ISRCs (if present)
* UPC/EAN (if present)

(That summary can be produced on its own using the `--no-rip` flag if that's all you're looking for.)



## Requirements

Rip Rip Hooray! is a command line utility built for and tested on x86-64 Linux systems, but it may well work with other 64-bit Unix platforms and/or Windows WSL too. See the [installation](#installation) section for information about building it from source.


### Optical Drive

Because of its focus on _recovery_, Rip Rip Hooray! imposes stricter requirements on optical drives than most CD-ripping software. To work with this program, your drive will need to support:

* Accurate Stream (most modern drives qualify)
* SCSI/MMC 2+ (again, most modern drives qualify)
* C2 Error Pointers

The drive will also need a known [read offset](http://www.accuraterip.com/driveoffsets.htm) to be auto-detected, or you'll need to know and enter the appropriate value using the `-o`/`--offset` option.

If your drive has a read buffer cache that isn't auto-detected, enter its size in kilobytes with the `-c`/`--cache` option so Rip Rip can try to mitigate its effects.

Programmatic detection of cache sizes is unreliable, so Rip Rip maintains its own manual list. To have your drive included, simply open an [issue](https://github.com/Blobfolio/riprip/issues) with the drive's vendor/model string — as displayed in the Rip Rip summary — along with a link to the manufacturer's spec page or manual (showing the buffer size).


### Disk/RAM

Unlike traditional CD-rippers, Rip Rip Hooray! can't just react to data in realtime and promptly throw it away; it needs to keep track of each individual sample's state and history to progressively work towards a complete rip.

This data is only needed while it's needed — you can delete the `_riprip` subfolder as soon as you've gotten what you wanted to reclaim the space — but is nonetheless hefty, generally about 1-3x the size of the original CD source.

Its peak memory usage is also higher than most other CD-rippers, though it varies based on the length of the longest track being ripped. A few hundred megabytes of RAM will usually suffice, but in worst-case scenarios like the 74-minute single-track album [Delirium Cordia](https://www.allmusic.com/album/delirium-cordia-mw0000693555) by Fantômas, nearly 3GiB will be required!


### Expectations

Rip Rip Hooray! does its best to _mitigate_ drive confusion and inconsistency, but like any other CD-ripping software, it is ultimately dependent on the drive's ability to accurately read the data on the disc, or at least be honest about any inaccuracies.

When a disc's surface is as pocked and cratered as the moon's, or disc rot has started to take hold, chances are some of that data will remain inaccessible, no matter how many times a drive, or multiple drives, attempts to re-read it.

(The unaffiliated) [CUETools](http://cue.tools/wiki/Main_Page)'s repair feature can be instrumental in filling in those final bits. If Rip Rip can't confirm the rips, toss them into CUETools to see if they're _close enough_ for automatic repair. If not, re-Rip Rip and try again.

Hopefully with a little back-and-forth, you'll wind up with perfect rips, one way or another.



## Usage

Rip Rip Hooray! is run from the command line, like:

```bash
riprip [OPTIONS]

# To see a list of options, use -h/--help:
riprip --help
```

### Example Recovery Workflow

First things first, rip the entire disc and see what happens!

Because Rip Rip Hooray! is optimized for _recovery_ rather than quick, efficient transfers, you may want to use a traditional — but _accuate_ — CD ripper for the first pass, like [fre:ac](https://github.com/enzo1982/freac/) or [EAC](https://www.exactaudiocopy.de/). Just be sure to disable their advanced error recovery features, or you'll be in for a _very long ride_. Haha.

From there, re-rip the problem tracks with Rip Rip:

```bash
# Say you need 2, 3, 4 and 10. Use the -t/--tracks argument.
riprip -t 2-4,10

# Equivalent alternatives:
# -t 2,3,4,10
# -t 2 -t 3 -t 4 -t 10
```

If you'd rather stick with one program to keep things simple, that's fine too. Rip Rip will rip an entire disc, including the HTOA (if any), by default, and generate a helpful cue sheet at the end of the process too:

```bash
# Rip the whole disc!
riprip
```

Whether you're ripping a few tracks or all tracks, Rip Rip will check them against both the [AccurateRip](http://accuraterip.com/) and [CUETools](http://cue.tools/wiki/CUETools_Database) databases to verify their accuracy. Confirmed tracks are exempted from subsequent rip passes, so aside from being perfect, they'll speed things up too.

If any tracks _don't_ verify after the initial Rip Rip rip, check to see if _enough_ data was recovered for [CUETools](http://cue.tools/wiki/CUETools) repair. You'll need the whole album for this, so if you used a different program for the good tracks, you'll need to merge those files with the ones Rip Rip partially recovered, otherwise you can just open the Rip Rip's cue sheet directly.

If automatic repair works, great! You're done!

If not, _iterate!_

Simply re-run Rip Rip. It will pick up from where it left off, (re)reading any sectors that have room for improvement, skipping the rest.

```bash
# Refine the original rip.
riprip

# If ripping specific tracks, keep being specific.
riprip -t 2-4,10
```

You can do this as many or as few times as needed. If you know you'll need several passes to get the data good enough for CUETools, you can automate them with the `-p`/`--passes` option, like:

```bash
# Run through each track up to three times, if needed.
riprip -p3

# Automation also allows for other fun things, like alternating between
# forward and backward traversal:
riprip -p3 --flip-flop
```

If problem tracks remain, recheck the refined album rip with CUETools repair. Rinse and repeat until everything is perfect, or the drive has clearly read everything it's ever going to read.

There are a number of different options that can come in handy for tricky situations, so be sure to take a look at the `--help` screen for inspiration.

Good luck!



## Installation

Debian and Ubuntu users can just grab the pre-built `.deb` package from the [release](https://github.com/Blobfolio/riprip/releases) page.

While specifically written for use on x86-64 Linux systems, both [Rust](https://www.rust-lang.org/) and [libcdio](https://www.gnu.org/software/libcdio/) are cross-platform, so you may well be able to build it from source on other 64-bit Unixish systems using `cargo`:

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
