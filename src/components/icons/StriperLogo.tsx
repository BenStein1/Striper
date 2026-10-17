import React from "react";
import striperLogo from "@/assets/striper-logo.png";

// The product name is a proper noun and is not translated.
const PRODUCT_NAME = "Striper";

/**
 * Striper logo lockup: the striped-bass artwork followed by the wordmark.
 * `width` is the overall width; the artwork and text scale with it.
 */
const StriperLogo = ({
  width = 200,
  className,
}: {
  width?: number;
  className?: string;
}) => {
  const iconSize = Math.round(width * 0.36);
  return (
    <div
      className={`flex items-center gap-2 select-none ${className ?? ""}`}
      style={{ width }}
    >
      <img
        src={striperLogo}
        alt=""
        aria-hidden="true"
        width={iconSize}
        height={iconSize}
        draggable={false}
      />
      <span
        className="font-semibold text-text leading-none tracking-tight"
        style={{ fontSize: Math.round(width * 0.2) }}
      >
        {PRODUCT_NAME}
      </span>
    </div>
  );
};

export default StriperLogo;
