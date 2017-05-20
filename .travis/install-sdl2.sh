#!/bin/sh
set -ex

if [ ! -d "$HOME/SDL2/lib" ]; then
	wget "https://www.libsdl.org/release/SDL2-2.0.5.tar.gz"
	tar xzf "SDL2-2.0.5.tar.gz"
	cd "SDL2-2.0.5"
	mkdir build
	cd build
	CC="gcc -m32" ../configure --host=i686-unknown-linux-gnu --prefix "$HOME/SDL2" --disable-{atomic,audio,render,events,joystick,haptic,power,filesystem,threads,timers,file,loadso,cpuinfo,assembly,video,shared}
	make -j2 && make install
	cd ../..
	rm -rf "SDL2-2.0.5" "SDL2-2.0.5.tar.gz"
else
	echo "Using cached directory."
fi
