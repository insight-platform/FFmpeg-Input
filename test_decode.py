from ffmpeg_input import FFMpegSource, VideoFrameEnvelope
import numpy as np
import cv2

if __name__ == '__main__':
    s = FFMpegSource("/home/ivan/video.mp4", {"c:v": "v4l2m2m"}, len=100, decode=True)
    while True:
        try:
            p = s.video_frame()
            res = np.reshape(np.frombuffer(bytes(p.payload), dtype=np.uint8), (p.frame_height, p.frame_width, 3))
            cv2.imshow('Image', res)
            if cv2.waitKey(1) & 0xFF == ord('q'):
                break
        except BrokenPipeError:
            print("EOS")
            break
