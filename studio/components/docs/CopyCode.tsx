"use client";

import { useState } from "react";

export function CopyCode({ value }: { value: string }) {
  const [copied, setCopied] = useState(false);

  async function copy() {
    await navigator.clipboard.writeText(value);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1600);
  }

  return <button className="docs-copy" type="button" onClick={copy} aria-label="Copy code to clipboard">
    {copied ? "Copied" : "Copy"}
  </button>;
}
