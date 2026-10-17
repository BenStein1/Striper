import React from "react";
import fishGlyph from "@/assets/striper-fish-glyph.png";

/**
 * Monochrome striped-bass glyph. The PNG is used as a CSS mask so the icon
 * takes the current text colour and follows the light/dark theme like the
 * other sidebar icons.
 */
const StriperFish = ({
  width,
  height,
  className,
}: {
  width?: number | string;
  height?: number | string;
  className?: string;
}) => {
  const mask = `url(${fishGlyph}) center / contain no-repeat`;
  return (
    <span
      aria-hidden="true"
      className={`inline-block bg-current ${className ?? ""}`}
      style={{
        width: width ?? 24,
        height: height ?? 24,
        WebkitMask: mask,
        mask,
      }}
    />
  );
};

export default StriperFish;
