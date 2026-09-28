import { useState } from "react";

import { cn } from "~/lib/utils";

const palette = ["#5e6ad2", "#4cb782", "#f2994a", "#eb5757", "#4ea7fc", "#bb87fc", "#26b5ce", "#f7c8c1"];

function colorFor(name: string) {
  let hash = 0;
  for (const character of name) hash = (hash * 31 + character.charCodeAt(0)) | 0;
  return palette[Math.abs(hash) % palette.length];
}

/** Round avatar that falls back to a tinted initial, like Linear's assignee chips. */
export function Avatar({
  name,
  src,
  size = 18,
  className,
  square = false,
}: {
  name: string;
  src?: string | null;
  size?: number;
  className?: string;
  square?: boolean;
}) {
  const [failed, setFailed] = useState(false);
  const shape = square ? "rounded-[5px]" : "rounded-full";
  if (src && !failed) {
    return (
      <img
        alt=""
        className={cn("shrink-0 bg-panel-subtle object-cover", shape, className)}
        height={size}
        onError={() => setFailed(true)}
        src={src}
        style={{ width: size, height: size }}
        width={size}
      />
    );
  }
  return (
    <span
      aria-hidden="true"
      className={cn("inline-grid shrink-0 place-items-center font-semibold uppercase text-white", shape, className)}
      style={{ width: size, height: size, backgroundColor: colorFor(name), fontSize: Math.max(8, Math.round(size * 0.45)) }}
    >
      {name.replace(/^@/, "").charAt(0) || "?"}
    </span>
  );
}

export function githubAvatar(login: string | null | undefined, size = 40) {
  return login ? `https://github.com/${encodeURIComponent(login)}.png?size=${size}` : null;
}
