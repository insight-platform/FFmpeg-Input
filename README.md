# FFmpeg Source Input Library

FFmpeg Source Input Library is a small library that makes it possible to receive the frames from
ffmpeg into python program. You can pass to ffmpeg required arguments and url and get the frames like
opencv does. The library provides the direct access to the ffmpeg library without the launching of 
the ffmpeg binary. The frames are in raw binary format and must be processed separately.

### Build In Docker (Manylinux_2_28)

```
docker build -t ffmpeg_input -f docker/Dockerfile.manylinux_2_28_X64 .
docker run --rm -it -v $(pwd)/distfiles:/tmp ffmpeg_input cp -R /opt/dist /tmp
```

### Try It

```
python3 test.py
# or
python3 test.py
```