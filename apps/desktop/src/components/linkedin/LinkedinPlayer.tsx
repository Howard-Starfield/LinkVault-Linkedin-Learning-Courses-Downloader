import type { CoursePlayback, PlayerSession, VideoPlayback, VideoProgress } from "../../lib/linkedin/types";
import { LinkedinPlayerPane } from "./LinkedinPlayerPane";

type LinkedinPlayerProps = {
  playback: CoursePlayback;
  session: PlayerSession;
  onClose: () => void;
  onProgress: (video: VideoPlayback, progress: VideoProgress) => void;
  onEnded: () => void;
};

export function LinkedinPlayer(props: LinkedinPlayerProps) {
  return <LinkedinPlayerPane {...props} />;
}
