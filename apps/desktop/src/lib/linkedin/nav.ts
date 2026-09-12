import type { LibraryNav, NavAction } from "./types";

export function reduceLibraryNav(nav: LibraryNav, action: NavAction): LibraryNav {
  switch (action.type) {
    case "openPath":
      return { level: "path", path: action.path };
    case "openCourse":
      return { level: "course", course: action.course, via: action.via };
    case "back":
      switch (nav.level) {
        case "catalog":
          return nav;
        case "path":
          return { level: "catalog" };
        case "course":
          return nav.via.kind === "path"
            ? { level: "path", path: nav.via.path }
            : { level: "catalog" };
        default: {
          const exhaustive: never = nav;
          return exhaustive;
        }
      }
    default: {
      const exhaustive: never = action;
      return exhaustive;
    }
  }
}
