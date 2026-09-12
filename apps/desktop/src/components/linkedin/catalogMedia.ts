export function catalogMediaKind(thumbnailUrl: string | null): "art" | "ring" {
  return thumbnailUrl !== null ? "art" : "ring";
}
