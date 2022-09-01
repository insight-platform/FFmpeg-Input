# FFmpeg-Input

FFmpeg Source Accessor Library

## Build In System

```
RUSTFLAGS=" -C target-cpu=native -C opt-level=3" maturin build --manylinux off --release --out dist --no-sdist
pip3 install --force-reinstall dist/ffmpeg_input-0.1.1-cp38-cp38-linux_x86_64.whl
```

### Build In Docker

```
docker build -t ffmpeg_input .
docker run --rm -it -v $(pwd)/distfiles:/tmp ffmpeg_input cp -R /opt/dist /tmp
pip3 install --force-reinstall distfiles/dist/*.whl
```

