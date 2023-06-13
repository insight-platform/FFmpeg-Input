# FFmpeg Source Input Library

FFmpeg Input Library is a small library aimed at receiving raw or decoded (RGB) frames from FFmpeg into a python program. 
You can pass to ffmpeg required arguments and url and get the frames like opencv does. The library 
provides the direct access to the ffmpeg library without the launching of the ffmpeg binary. 
The frames are in raw binary format and must be processed separately.


### Install Prebuilt Wheels

```bash
pip install ffmpeg-input 
```

### Build In Docker (Manylinux_2_28)

```bash
docker build -t ffmpeg_input -f docker/Dockerfile.manylinux_2_28_X64 .
docker run --rm -it -v $(pwd)/distfiles:/tmp ffmpeg_input cp -R /opt/dist /tmp
```

### Try It

```
RUST_LOG=debug python3 test.py
```