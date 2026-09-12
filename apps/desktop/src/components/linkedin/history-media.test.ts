import { catalogMediaKind } from "./catalogMedia";

if (catalogMediaKind("https://media.example/thumb.jpg") !== "art") {
  throw new Error("MiniCourseArt should render when thumbnailUrl is a string");
}
if (catalogMediaKind(null) !== "ring") {
  throw new Error("MiniCourseArt is absent when thumbnailUrl === null");
}
