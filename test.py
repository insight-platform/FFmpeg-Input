from time import sleep

from ffmpeg_input import FFMpegSource, FFmpegLogLevel

if __name__ == '__main__':
    s = FFMpegSource("/home/ivan/file1.mp4", params={},
                     queue_len=10, decode=False,
                     block_if_queue_full=True,
                     ffmpeg_log_level=FFmpegLogLevel.Debug)
    s.log_level = FFmpegLogLevel.Trace
    while True:
        try:
            p = s.video_frame()
            print("Codec: {}, Geometry: {}x{}".format(p.codec, p.frame_width, p.frame_height))
            print("System ts, when the frame was received from the source:", p.frame_received_ts)
            print("Current queue length:", p.queue_len)
            print("Time base, pts, dts:", p.time_base, p.pts, p.dts)
            print("Skipped frames because of queue overflow:", p.queue_full_skipped_count)
            print("Payload length:", len(p.payload_as_bytes()))
        except BrokenPipeError:
            print("EOS")
            break
