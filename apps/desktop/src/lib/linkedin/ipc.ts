import { invoke } from "@tauri-apps/api/core";

export type LinkedInDestinationCommit<TBootstrap> = {
  outputDir: string;
  imported: number;
  skipped: number;
  alreadyKnown: number;
  bootstrap: TBootstrap;
};

export async function commitLinkedInDestination<TBootstrap>(
  path: string
): Promise<LinkedInDestinationCommit<TBootstrap>> {
  return invoke<LinkedInDestinationCommit<TBootstrap>>("commit_linkedin_destination", { path });
}
