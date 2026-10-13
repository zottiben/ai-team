import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { readImages, ImageDrafts, SentImages } from "./ChatImages";

const service = vi.hoisted(() => ({ api: vi.fn() }));
vi.mock("./api", () => service);
afterEach(() => { cleanup(); vi.clearAllMocks(); });
const image = { name: "screen.png", media_type: "image/png", data: "aW1hZ2U=" };

it("bounds file selection and rejects unsupported types without reading them", async () => {
  const file = new File(["image"], "screen.png", { type: "image/png" });
  await expect(readImages([file], 4)).rejects.toThrow("at most 4");
  await expect(readImages([new File(["<svg/>"], "vector.svg", { type: "image/svg+xml" })], 0)).rejects.toThrow("Other files");
  await expect(readImages([new File([], "empty.png", { type: "image/png" })], 0)).rejects.toThrow("1 byte");
  await expect(readImages([new File([new Uint8Array(5 * 1024 * 1024 + 1)], "large.png", { type: "image/png" })], 0)).rejects.toThrow("5 MiB");
  await expect(readImages([file], 0)).resolves.toEqual([image]);
});

it("previews locally and removes exactly the selected draft image", () => {
  const remove = vi.fn();
  render(<ImageDrafts images={[image, { ...image, name: "other.png" }]} disabled={false} onRemove={remove}/>);
  expect(screen.getByAltText("Attached screen.png").getAttribute("src")).toBe("data:image/png;base64,aW1hZ2U=");
  fireEvent.click(screen.getByRole("button", { name: "Remove image 2: other.png" }));
  expect(remove).toHaveBeenCalledWith(1);
  expect(service.api).not.toHaveBeenCalled();
});

it("loads stored images only on demand using their exact chat, node and index", async () => {
  service.api.mockResolvedValue(image);
  render(<SentImages chat={7} node={23} images={[{ index: 2, name: image.name, media_type: image.media_type, bytes: 5 }]}/>);
  expect(service.api).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "View image" }));
  expect(await screen.findByAltText("screen.png")).toBeTruthy();
  expect(service.api).toHaveBeenCalledWith("/chats/7/images/23/2");
});
