import { LaunchProps, open, showHUD, showToast, Toast } from "@raycast/api";
import { readRadar, StateFileMissingError } from "./lib/readProjects.ts";
import { resolveProjectPath } from "./lib/resolvePath.ts";

export default async function Command(
  props: LaunchProps<{ arguments: { query: string } }>,
) {
  const { query } = props.arguments;
  try {
    const radar = readRadar();
    const path = resolveProjectPath(radar.projects, query);
    if (path === null) {
      await showHUD(`No project matches "${query}"`);
      return;
    }
    await open(path);
  } catch (err) {
    const message =
      err instanceof StateFileMissingError ? err.message : String(err);
    await showToast({ style: Toast.Style.Failure, title: "petri", message });
  }
}
