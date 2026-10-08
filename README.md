# Rip Rip Hooray!

[![ci](https://img.shields.io/github/actions/workflow/status/Blobfolio/riprip/ci.yaml?style=flat-square&label=ci)](https://github.com/Blobfolio/riprip/actions)
[![deps.rs](https://deps.rs/repo/github/blobfolio/riprip/status.svg?style=flat-square&label=deps.rs)](https://deps.rs/repo/github/blobfolio/riprip)<br>
[![license](https://img.shields.io/badge/license-wtfpl-ff1493?style=flat-square)](https://en.wikipedia.org/wiki/WTFPL)
[![contributions welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg?style=flat-square&label=contributions)](https://github.com/Blobfolio/riprip/issues)


Rip Rip Hooray! is a specialized audio CD-ripper for Linux/Mac that is optimized for _track recovery_.

<img src="https://github.com/Blobfolio/riprip/raw/master/riprip_core/skel/riprip-a.webp" width="30%" alt="Rip Rip rip settings confirmation screen."></img> <img src="https://github.com/Blobfolio/riprip/raw/master/riprip_core/skel/riprip-b.webp" width="60%" alt="Rip Rip rip settings confirmation screen."></img>

Rather than beat the drive senseless every time a read error is encountered, it simply notes the problem and moves on. Its iterative design allows it to grab what it can, as it can, progressively filling in the gaps from run-to-run.

Between those (relatively quick) runs, you can actually _do things_. You can inspect the disc, give it another clean, switch drives, shut down your computer and go to bed, or check to see if the rip is already _good enough_ for [CUETools repair](https://cue.tools/wiki/CUETools_Database) to automatically finish up for you.

Total recovery is not always possible — drives can only read what they can read — but Rip Rip Hooray! will rescue more data than traditional CD-ripping software, more accurately, and in significantly less time.



## Features

Iteration is key. Individual Rip Rip rips take minutes intead of hours or days, getting you access to the recovered data — regardless of "completeness" — as quickly as possible. You can re-run Rip Rip at any time, as many times as you want, with as many different optical drives as you want, to retry the outstanding regions and refine the data. You can also abort a rip early without losing your progress.

Beyond that, it supports all the good things:

* C2 error pointers
* Subchannel timecode synchronization
* Drive read offset auto-detection and correction
* [AccurateRip](https://accuraterip.com/) and [CUETools](https://cue.tools/wiki/CUETools_Database) checksum verification
* HTOA (pregap) tracks
* CD-Text and Sub-Q metadata
* Cache busting
* Sample re/confirmation
* Forwards, backwards, and alternating read orders
* Cue sheet generation in both `.cue` and `.toc` formats (whole album rips only)
* Simple, honest `.wav` output

Rip Rip can also be used to summarize a disc's track layout, and calculate and display its various TOC-derived identifiers, including:

* [AccurateRip](https://accuraterip.com/) ID
* [CDDB](https://en.wikipedia.org/wiki/CDDB) ID
* `"CDTOC"` (metatag value)
* [CUETools](https://cue.tools/wiki/CUETools_Database) ID
* [MusicBrainz](https://musicbrainz.org/) ID



## Requirements

Rip Rip Hooray! is a command line application for 64-bit Linux and Mac systems. It's fully self-contained and has no runtime dependencies.


### Optical Drive

Because of its focus on _recovery_, Rip Rip Hooray! imposes stricter requirements on optical drives than most CD-ripping software. To work with this program, your drive will need to support:

* Accurate Stream (most modern drives qualify)
* SCSI/MMC 2+ (again, most modern drives qualify)
* C2 Error Pointers

The drive will also need a known [read offset](https://www.accuraterip.com/driveoffsets.htm) to be auto-detected, or you'll need to know and enter the appropriate value using the `-o`/`--offset` option.

If your drive has a read buffer cache that isn't auto-detected, enter its size in **kilobytes** with the `-c`/`--cache` option so Rip Rip can try to mitigate its effects.

Programmatic detection of cache size is unreliable, so Rip Rip maintains its own manual list. To have your drive included, simply open an [issue](https://github.com/Blobfolio/riprip/issues) with your drive's vendor/model string — as displayed by Rip Rip — and a link to a manual/spec page showing the value.


### Disk/RAM

Unlike traditional CD-rippers, Rip Rip Hooray! can't just react to data in realtime and promptly throw it away; it needs to keep track of each individual sample's state and history to progressively work towards a complete rip.

This data is only needed while it's needed — you can delete the `_riprip` subfolder as soon as you've gotten what you wanted to reclaim the space — but is nonetheless hefty, generally about 1-3x the size of the original CD source.

Its peak memory usage is also higher than most other CD-rippers, though it varies based on the length of the longest track being ripped. A few hundred megabytes of RAM will usually suffice, but in worst-case scenarios like the 74-minute single-track album [Delirium Cordia](https://www.allmusic.com/album/delirium-cordia-mw0000693555) by Fantômas, nearly 3GiB will be required!


### Expectations

Rip Rip Hooray! does its best to _mitigate_ drive confusion and inconsistency, but like any other CD-ripping software, it is ultimately dependent on the drive's ability to accurately read the data on the disc, or at least be honest about any inaccuracies.

When a disc's surface is as pocked and cratered as the moon's, or disc rot has started to take hold, chances are some of that data will remain inaccessible, no matter how many times a drive, or multiple drives, attempts to re-read it.

(The unaffiliated) [CUETools](http://cue.tools/wiki/Main_Page)'s repair feature can be instrumental in filling in those final bits. If Rip Rip can't confirm the rips, toss them into CUETools to see if they're _close enough_ for automatic repair. If not, run another Rip Rip pass and try again. Rinse and repeat.

Hopefully with a little back-and-forth, you'll wind up with perfect rips, one way or another!



## Usage

Rip Rip Hooray! is run from the command line, like:

```bash
riprip [OPTIONS]

# To see a list of options, use -h/--help:
riprip --help
```


### Example Recovery Workflow

First things first, rip the entire disc and see what happens!

```bash
# Rip the whole disc!
riprip
```

Rip Rip will check each track against both the [AccurateRip](https://accuraterip.com/) and [CUETools](https://cue.tools/wiki/CUETools_Database) databases to verify its accuracy, letting you know which were rescued, and which need more work.

If _all_ tracks verify, hooray! You're done!

If not, try opening the generated `.cue` with [CUETools](http://cue.tools/wiki/CUETools) to see if it can automatically "repair" the rip for you. (Linux users can [run CUETools via WINE](https://blobfolio.com/2023/cuetools-wine/).)

If that works, hooray! You're done!

If not, _iterate!_

Simply re-run Rip Rip to refine the results. It will pick up from where it left off, (re)reading any sectors that have room for improvement, skipping the rest.

```bash
# Re-rip!
riprip
```

Same as before, if any problem tracks remain, open up CUETools to see if they're accurate _enough_ for automatic "repair".

Rinse and repeat!

If you know you'll need several more passes to get the data good enough for CUETools, you can automate them with the `-p`/`--passes` option, like:

```bash
# Run through each track up to three times, if needed.
riprip -p3

# Automation also allows for other fun things, like alternating between
# forward and backward traversal:
riprip -p3 --flip-flop
```

Sooner or later, you'll either wind up with everything, or everything the drive can possibly deliver.

There are a number of advanced options that can come in handy for tricky situations, so be sure to take a look at the `--help` screen for inspiration.

Good luck!


### Debugging and Troubleshooting.

If a disc is giving you trouble or you simply want a record of the proceedings, set the `-v`/`--verbose` flag to have Rip Rip emit a log to STDOUT.

```bash
# The normal interface uses STDERR, so to keep the two from getting tangled,
# redirect the log to e.g. a file:
riprip -v > my-rip.log

# For additional information, set -v twice.
riprip -v -v > my-rip.log
```

If you're experiencing issues with the drive or Rip Rip itself, another `-v` or two can help with debugging:

```bash
# Trace-level reporting gives more information about what is failing
# and where.
riprip -v -v -v > my-rip.log

# Trace-Premium-Plus™ gives context to the context, enumerating the
# specific values, settings, etc., in play at the moment of failure.
riprip -v -v -v -v > my-rip.log
```

Optical drives and drivers are weird, so please don't hesitate to open an [issue](https://github.com/Blobfolio/riprip/issues/new) if something's not working right!



## Installation

Pre-built x86-64-v3 `.deb` packages for Debian and Ubuntu are attached to the [latest release](https://github.com/Blobfolio/riprip/releases/latest), and Arch Linux users can install the same through [AUR](https://aur.archlinux.org/packages/riprip-bin) (thanks @Dominiquini!).

To get Rip Rip up and running on other 64-bit Linux and Mac systems, it just needs to be built from source.

There are only three pre-requisites:

* [Rust](https://rustup.rs/) (latest stable)
* `clang` or `gcc`
* [`cmake`](https://github.com/aws/aws-lc-rs/blob/main/aws-lc-sys/CMakeLists.txt#L4C32-L4C42)

> [!NOTE]
> 
> Apple users will additionally need to configure Xcode.
> 
> ```bash
> # Install Apple's command line tools.
> xcode-select --install
> 
> # The above should give you clang, among other things.
> clang --version
> ```

Once you've got those things squared away, a single command'll do the trick:

```bash
# See "cargo install --help" for more options.
cargo install \
    --git https://github.com/Blobfolio/riprip.git \
    --bin riprip

# Hooray?
riprip --version
```

If you run into any issues or would like to help make Rip Rip available on more platforms, [let us know](https://github.com/Blobfolio/riprip/issues/new)!
