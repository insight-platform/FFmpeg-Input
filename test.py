from ffmpeg_input import FFMpegSource, VideoFrameEnvelope

if __name__ == '__main__':
    s = FFMpegSource("/dev/video0", params={"video_size": "320x240"}, len=100)
    #s = FFMpegSource("/home/ivan/video.mp4", {})
    while True:
        try:
            p = s.video_frame()
            print(p.frame_width)
            print(p.frame_height)
            print(p.codec)
            print(p.system_ts)
            print(p.queue_len)
        except BrokenPipeError:
            print("EOS")
            break
