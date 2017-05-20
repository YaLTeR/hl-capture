#!/bin/sh
set -ex

if [ ! -d "$HOME/ffmpeg/lib" ]; then
	git clone https://github.com/FFMpeg/FFMpeg.git "$HOME/ffmpeg_src"
	mkdir "$HOME/ffmpeg_build"
	cd "$HOME/ffmpeg_build"
	../ffmpeg_src/configure --disable-programs --disable-doc --enable-cross-compile --arch=x86_32 --target_os=linux --prefix="$HOME/ffmpeg" --cc="gcc -m32" --disable-static --enable-shared
	make -j && make install
	cd ..
	rm -rf ffmpeg_src ffmpeg_build
else
	echo "Using cached directory."
fi
