# hl-capture

[![Build Status](https://travis-ci.org/YaLTeR/hl-capture.svg?branch=master)](https://travis-ci.org/YaLTeR/hl-capture)

[Internal rustdoc](https://yalter.github.io/hl-capture)

hl-capture is a tool for recording Half-Life videos on Linux, written in Rust. It's similar to [Half-Life Advanced Effects](http://www.advancedfx.org/), but focuses on video capturing rather than advanced movie-making functionality.

hl-capture is designed to be **fast** and **convenient**. Video and sound are encoded with [FFMpeg](http://ffmpeg.org/) right away into any desirable format like `mp4`, `mkv` or `webm`. This, together with utilizing multiple threads and GPU-accelerated processing, makes hl-capture way faster than HLAE or Source's startmovie.

## Features
- Fast video and sound capturing and encoding into almost any of the formats supported by FFMpeg.
- GPU-accelerated resampling.
- TAS compatibility out of the box, including engine restarts.

## Requirements
- 32-bit FFMpeg libraries.
- 32-bit SDL2.
- 32-bit OpenCL.

## Usage
First of all, make sure **not** to connect to multiplayer servers with hl-capture loaded, as this could cause a VAC ban.

Download or build `libhl_capture.so`. Get the full path to the folder where you put it (for example, by running `pwd` from the terminal in that folder).

The easiest way of loading hl-capture is going into Half-Life's properties by right clicking it in Steam's game list, then pressing Set Launch Options, and entering `LD_PRELOAD=/full/path/to/your/libhl_capture.so %command%`. Launch Half-Life through Steam and verify that hl-capture is loaded by checking that `cap_` console commands and variables exist. Make sure to clear the launch options after you're done using hl-capture to not get VAC banned accidentally by connecting to a multiplayer server with hl-capture loaded.

Another way is by using a shell script to run Half-Life. Download [this example script](https://gist.github.com/YaLTeR/262d60eef7933f8c61e122cde0c548cb), change the variables inside appropriately, mark it as executable. Then launch Half-Life with hl-capture by running the shell script.

## Goals
- High capturing speed.
- Compatibility with TASes, including those utilizing engine restarts and RNG manipulation.

## Building
1. Get stable Rust, then do `rustup target add i686-unknown-linux-gnu`
2. Install **32-bit** FFMpeg libraries.
3. Install **32-bit** SDL2.
4. Install **32-bit** OpenCL.
5. `PKG_CONFIG_ALLOW_CROSS=1 cargo build --release`

Look at how the Travis build is set up for minimal build of the dependencies.
