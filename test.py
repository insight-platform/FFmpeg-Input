from ffmpeg_input import FFMpegSource, VideoFrameEnvelope
import numpy as np
import cv2

if __name__ == '__main__':
    s = FFMpegSource("/dev/video0", params={"video_size": "320x240", "c:v": "v4l2m2m"}, len=100, decode=False)
    while True:
        try:
            p = s.video_frame()
            print("Codec: {}, Geometry: {}x{}".format(p.codec, p.frame_width, p.frame_height))
            print("System ts, when the frame was received from the source:", p.frame_received_ts)
            print("Current queue length:", p.queue_len)
            print("Skipped frames because of queue overflow:", p.queue_full_skipped_count)
            print("Payload length:", len(p.payload_as_bytes()))
        except BrokenPipeError:
            print("EOS")
            break
