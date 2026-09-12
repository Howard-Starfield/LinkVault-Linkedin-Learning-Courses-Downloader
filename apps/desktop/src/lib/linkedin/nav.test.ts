import { reduceLibraryNav } from "./nav";
import { asCourseSlug, asPathSlug } from "./types";
import type { LibraryNav } from "./types";

const path = asPathSlug("github-cert");
const course = asCourseSlug("practical-github-actions");

const fromCatalog: LibraryNav = { level: "catalog" };
const pathNav = reduceLibraryNav(fromCatalog, { type: "openPath", path });
const courseViaPath = reduceLibraryNav(pathNav, {
  type: "openCourse",
  course,
  via: { kind: "path", path }
});
const backToPath = reduceLibraryNav(courseViaPath, { type: "back" });
const backToCatalog = reduceLibraryNav(backToPath, { type: "back" });
const standaloneCourse = reduceLibraryNav(fromCatalog, {
  type: "openCourse",
  course,
  via: { kind: "standalone" }
});
const standaloneBack = reduceLibraryNav(standaloneCourse, { type: "back" });
const catalogBack = reduceLibraryNav(fromCatalog, { type: "back" });

if (pathNav.level !== "path" || pathNav.path !== path) {
  throw new Error("openPath should enter the path level");
}
if (courseViaPath.level !== "course" || courseViaPath.via.kind !== "path") {
  throw new Error("openCourse from a path should keep CourseVia");
}
if (backToPath.level !== "path" || backToPath.path !== path) {
  throw new Error("back from a path course should restore the path");
}
if (backToCatalog.level !== "catalog") {
  throw new Error("back from a path should restore the catalog");
}
if (standaloneBack.level !== "catalog") {
  throw new Error("back from a standalone course should restore the catalog");
}
if (catalogBack.level !== "catalog") {
  throw new Error("back on catalog should stay on catalog");
}
