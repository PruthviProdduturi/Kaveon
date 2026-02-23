"use client";

import { useState, useEffect } from "react";
import { LoomXLoading } from "./LoomXLoading";

export function LoadingOverlay() {
  const [isVisible, setIsVisible] = useState(true);

  useEffect(() => {
    // Ensure the loading overlay is fully visible before potentially unmounting
    setIsVisible(true);
  }, []);

  return (
    <div
      style={{
        opacity: isVisible ? 1 : 0,
        transition: 'opacity 0.3s ease-in-out',
      }}
    >
      <LoomXLoading fullScreen={true} />
    </div>
  );
}
