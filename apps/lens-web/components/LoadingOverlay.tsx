"use client";

import { useState, useEffect } from "react";
import { LensLoading } from "./LensLoading";

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
      <LensLoading fullScreen={true} />
    </div>
  );
}
