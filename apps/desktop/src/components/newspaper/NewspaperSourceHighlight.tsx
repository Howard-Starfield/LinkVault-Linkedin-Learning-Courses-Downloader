import type { NormalizedCropRect } from "./newspaper-api";

export function NewspaperSourceHighlight({ rect }: { rect: NormalizedCropRect }) {
  return (
    <div
      aria-hidden="true"
      className="newspaper-source-highlight"
      data-testid="newspaper-source-highlight"
      style={{
        left: `${rect.x * 100}%`,
        top: `${rect.y * 100}%`,
        width: `${rect.width * 100}%`,
        height: `${rect.height * 100}%`
      }}
    />
  );
}
