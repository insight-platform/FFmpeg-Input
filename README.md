# FFmpeg Source Input Library

FFmpeg Source Input Library is a small library that makes it possible to receive the frames from
ffmpeg into python program. You can pass to ffmpeg required arguments and url and get the frames like
opencv does. The library provides the direct access to the ffmpeg library without the launching of 
the ffmpeg binary. The frames are in raw binary format and must be processed separately.

## Build In System

```
RUSTFLAGS=" -C target-cpu=native -C opt-level=3" maturin build --manylinux off --release --out dist --no-sdist
pip3 install --force-reinstall dist/ffmpeg_input-0.1.1-cp38-cp38-linux_x86_64.whl
```

### Build In Docker

The base image is Python 3.8 / 3.9. If you have another Python version, change the image accordingly. The versions must match.

```
docker build -t ffmpeg_input -f Dockerfile.38 .
docker run --rm -it -v $(pwd)/distfiles:/tmp ffmpeg_input cp -R /opt/dist /tmp
pip3 install --force-reinstall distfiles/dist/*.whl
```

### Try It

```
python3 test.py
# or
RUST_LOG=debug python3 test.py
```