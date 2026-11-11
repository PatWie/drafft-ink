import { PNG } from "pngjs";

/** A solid-colour PNG, as base64, for drops and `image.insert`. */
export function solidPng(width: number, height: number, rgb: readonly [number, number, number]): string {
  const png = new PNG({ width, height });
  for (let i = 0; i < width * height; i++) {
    png.data[i * 4] = rgb[0];
    png.data[i * 4 + 1] = rgb[1];
    png.data[i * 4 + 2] = rgb[2];
    png.data[i * 4 + 3] = 255;
  }
  return PNG.sync.write(png).toString("base64");
}
