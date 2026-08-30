export type DestinationPath = string & { readonly __destination: unique symbol };

export function parseDestination(raw: string | null | undefined): DestinationPath | null {
  const trimmed = raw?.trim() ?? "";
  if (!trimmed || trimmed === "." || trimmed === "./") {
    return null;
  }
  return trimmed as DestinationPath;
}

export async function ensureDestination(args: {
  current: string | DestinationPath | null | undefined;
  ask: () => Promise<string | null>;
}): Promise<DestinationPath | null> {
  const existing = parseDestination(args.current);
  if (existing) {
    return existing;
  }
  return parseDestination(await args.ask());
}
