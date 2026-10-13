import { useEffect, useState } from "react";
import { api } from "./api";
import type { ImageInput, ImageInfo } from "./chat-api";

const TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp"];
export async function readImages(files: File[], existing: number): Promise<ImageInput[]> {
  if (existing + files.length > 4) throw new Error("Attach at most 4 images per message.");
  for (const file of files) {
    if (!TYPES.includes(file.type)) throw new Error("Choose PNG, JPEG, GIF or WebP images. Other files are not supported.");
    if (!file.size || file.size > 5 * 1024 * 1024) throw new Error("Each image must contain 1 byte to 5 MiB.");
  }
  return Promise.all(files.map(file => new Promise<ImageInput>((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error(`Could not read ${file.name}. Your message is kept.`));
    reader.onload = () => resolve({ name: file.name, media_type: file.type, data: String(reader.result).split(",")[1] ?? "" });
    reader.readAsDataURL(file);
  })));
}

export function ImageDrafts({ images, disabled, onRemove }: { images: ImageInput[]; disabled: boolean; onRemove: (index: number) => void }) {
  return <div className="chat-images" aria-label="Image attachments">{images.map((image, index) => <figure key={index}>
    <img src={`data:${image.media_type};base64,${image.data}`} alt={`Attached ${image.name}`} />
    <figcaption>{image.name}</figcaption>
    <button type="button" className="button" disabled={disabled} onClick={() => onRemove(index)} aria-label={`Remove image ${index + 1}: ${image.name}`}>Remove</button>
  </figure>)}</div>;
}

export function SentImages({ chat, node, images }: { chat: number; node: number; images: ImageInfo[] }) {
  return <div className="chat-images">{images.map(image => <SavedImage key={image.index} chat={chat} node={node} image={image}/>)}</div>;
}
function SavedImage({ chat, node, image }: { chat: number; node: number; image: ImageInfo }) {
  const [open, setOpen] = useState(false);
  const [source, setSource] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => {
    if (!open) return;
    let current = true;
    void api<ImageInput>(`/chats/${chat}/images/${node}/${image.index}`).then(value => {
      if (current) setSource(`data:${value.media_type};base64,${value.data}`);
    }).catch((error: unknown) => { if (current) setProblem(error instanceof Error ? error.message : String(error)); });
    return () => { current = false; };
  }, [chat, node, image.index, open]);
  return <figure>{source && <img src={source} alt={image.name}/>}<figcaption>{image.name}{open && !source && !problem && " · loading image…"}</figcaption>{!open && <button type="button" className="button" onClick={() => setOpen(true)}>View image</button>}{problem && <p className="error">{problem}</p>}</figure>;
}
