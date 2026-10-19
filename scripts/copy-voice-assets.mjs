// Copies the voice-activity-detector files (the Silero model and its audio worklet) into public/vad so they are served by Leo itself and
// the microphone starts without any network access. Run before dev and build.
import { copyFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";

const out = join("public", "vad");
mkdirSync(out, { recursive: true });
const vad = join("node_modules", "@ricky0123", "vad-web", "dist");
const files = [
  [vad, "silero_vad_v5.onnx"],
  [vad, "vad.worklet.bundle.min.js"],
];
for (const [dir, name] of files) copyFileSync(join(dir, name), join(out, name));
console.log(`voice assets copied to ${out}`);
