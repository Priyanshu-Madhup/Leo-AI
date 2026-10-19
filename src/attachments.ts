/**
 * Images attached to a chat message. Whatever the picture's format, it is
 * decoded by the window's own engine, scaled down and re-encoded as JPEG, so
 * the model always receives something it accepts and a phone photo does not
 * become a 10 MB upload.
 */

export interface Attachment {
  id: number;
  dataUrl: string;
  name: string;
}

export const MAX_ATTACHMENTS = 6;
const MAX_SIDE = 2048;
const MAX_CHARS = 5_000_000;

const IMAGE_NAME = /\.(png|jpe?g|jfif|gif|webp|avif|bmp|svg|ico|tiff?|heic|heif)$/i;

export function isImageFile(file: File): boolean {
  return file.type.startsWith("image/") || IMAGE_NAME.test(file.name);
}

interface Decoded {
  source: CanvasImageSource;
  width: number;
  height: number;
  release(): void;
}

async function decode(file: File): Promise<Decoded> {
  try {
    const bitmap = await createImageBitmap(file);
    return { source: bitmap, width: bitmap.width, height: bitmap.height, release: () => bitmap.close() };
  } catch {
    // SVG and a few other formats only decode through an <img>.
    const url = URL.createObjectURL(file);
    try {
      const img = new Image();
      img.src = url;
      await img.decode();
      return {
        source: img,
        width: img.naturalWidth || 800,
        height: img.naturalHeight || 800,
        release: () => URL.revokeObjectURL(url),
      };
    } catch (err) {
      URL.revokeObjectURL(url);
      throw err;
    }
  }
}

/** Decodes, shrinks and re-encodes one image file. Rejects if the format cannot be read. */
export async function prepareImage(file: File, id: number): Promise<Attachment> {
  const decoded = await decode(file);
  try {
    let scale = Math.min(1, MAX_SIDE / Math.max(decoded.width, decoded.height));
    for (let attempt = 0; attempt < 4; attempt++) {
      const canvas = document.createElement("canvas");
      canvas.width = Math.max(1, Math.round(decoded.width * scale));
      canvas.height = Math.max(1, Math.round(decoded.height * scale));
      const ctx = canvas.getContext("2d")!;
      // JPEG has no transparency: a see-through PNG would turn black.
      ctx.fillStyle = "#fff";
      ctx.fillRect(0, 0, canvas.width, canvas.height);
      ctx.drawImage(decoded.source, 0, 0, canvas.width, canvas.height);
      const dataUrl = canvas.toDataURL("image/jpeg", attempt === 0 ? 0.88 : 0.75);
      if (dataUrl.length <= MAX_CHARS || attempt === 3) {
        if (dataUrl.length > MAX_CHARS) throw new Error("too large");
        return { id, dataUrl, name: file.name || "image" };
      }
      scale *= 0.7;
    }
    throw new Error("too large");
  } finally {
    decoded.release();
  }
}

/** The row of thumbnails above the message box. */
export class AttachmentTray {
  private items: Attachment[] = [];
  private nextId = 1;
  private pending = 0;

  constructor(
    private tray: HTMLElement,
    private onChange: () => void,
    private notify: (message: string) => void,
  ) {}

  get count(): number {
    return this.items.length;
  }

  /** True while images are still being converted (sending now would leave them out). */
  get busy(): boolean {
    return this.pending > 0;
  }

  dataUrls(): string[] {
    return this.items.map((i) => i.dataUrl);
  }

  clear(): void {
    this.items = [];
    this.render();
  }

  async add(files: Iterable<File>): Promise<void> {
    const list = [...files];
    const images = list.filter(isImageFile);
    if (list.length > images.length) this.notify("Only images can be attached.");
    for (const file of images) {
      if (this.items.length + this.pending >= MAX_ATTACHMENTS) {
        this.notify(`You can attach up to ${MAX_ATTACHMENTS} images.`);
        break;
      }
      this.pending++;
      this.onChange();
      try {
        this.items.push(await prepareImage(file, this.nextId++));
        this.render();
      } catch {
        this.notify(`Couldn't read ${file.name || "that image"}. Try saving it as PNG or JPEG.`);
      } finally {
        this.pending--;
        this.onChange();
      }
    }
  }

  private render(): void {
    this.tray.replaceChildren(
      ...this.items.map((item) => {
        const thumb = document.createElement("div");
        thumb.className = "thumb";
        const img = document.createElement("img");
        img.src = item.dataUrl;
        img.alt = item.name;
        const remove = document.createElement("button");
        remove.type = "button";
        remove.setAttribute("aria-label", `Remove ${item.name}`);
        remove.textContent = "×";
        remove.addEventListener("click", () => {
          this.items = this.items.filter((i) => i.id !== item.id);
          this.render();
        });
        thumb.append(img, remove);
        return thumb;
      }),
    );
    this.tray.classList.toggle("hidden", this.items.length === 0);
    this.onChange();
  }
}

/** Shows sent images inside a message bubble. */
export function renderBubbleImages(bubble: HTMLElement, urls: string[]): void {
  if (urls.length === 0) return;
  const row = document.createElement("div");
  row.className = "msg-images";
  for (const url of urls) {
    const img = document.createElement("img");
    img.src = url;
    img.alt = "Attached image";
    row.append(img);
  }
  bubble.prepend(row);
  bubble.classList.add("has-images");
}
