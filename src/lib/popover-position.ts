type PopoverPlacement = "above" | "below" | "auto";
type PopoverAlign = "start" | "end" | "center";

type PopoverPositionOptions = {
  align?: PopoverAlign;
  gap?: number;
  margin?: number;
  maxWidth?: number;
  minHeight?: number;
  minWidth?: number;
  placement?: PopoverPlacement;
  preferredWidth?: number;
};

function clamp(value: number, minimum: number, maximum: number) {
  return Math.min(Math.max(value, minimum), maximum);
}

export function anchoredPopoverStyle(
  trigger: HTMLElement,
  popover: HTMLElement,
  options: PopoverPositionOptions = {}
) {
  const viewport = window.visualViewport;
  const viewportLeft = viewport?.offsetLeft ?? 0;
  const viewportTop = viewport?.offsetTop ?? 0;
  const viewportWidth = viewport?.width ?? window.innerWidth;
  const viewportHeight = viewport?.height ?? window.innerHeight;
  const margin = options.margin ?? 12;
  const gap = options.gap ?? 10;
  const triggerRect = trigger.getBoundingClientRect();
  const popoverRect = popover.getBoundingClientRect();
  const availableWidth = Math.max(0, viewportWidth - margin * 2);
  const minimumWidth = Math.min(options.minWidth ?? 240, availableWidth);
  const preferredWidth = options.preferredWidth ?? (popoverRect.width || triggerRect.width);
  const width = clamp(preferredWidth, minimumWidth, Math.min(options.maxWidth ?? availableWidth, availableWidth));

  let left = triggerRect.left;
  if (options.align === "end") {
    left = triggerRect.right - width;
  } else if (options.align === "center") {
    left = triggerRect.left + (triggerRect.width - width) / 2;
  }
  left = clamp(left, viewportLeft + margin, viewportLeft + viewportWidth - width - margin);

  const aboveSpace = triggerRect.top - viewportTop - gap - margin;
  const belowSpace = viewportTop + viewportHeight - triggerRect.bottom - gap - margin;
  const preferredPlacement = options.placement === "above" || options.placement === "below" ? options.placement : null;
  const minimumUsefulHeight = Math.min(options.minHeight ?? 120, viewportHeight - margin * 2);
  const preferredSpace = preferredPlacement === "above" ? aboveSpace : belowSpace;
  const alternateSpace = preferredPlacement === "above" ? belowSpace : aboveSpace;
  const placement =
    preferredPlacement && (preferredSpace >= minimumUsefulHeight || preferredSpace >= alternateSpace)
      ? preferredPlacement
      : preferredPlacement
        ? preferredPlacement === "above"
          ? "below"
          : "above"
        : belowSpace >= popoverRect.height || belowSpace >= aboveSpace
          ? "below"
          : "above";
  const availableHeight = Math.max(0, Math.min(placement === "above" ? aboveSpace : belowSpace, viewportHeight - margin * 2));
  const renderedHeight = Math.min(popoverRect.height, availableHeight);
  let top = placement === "above" ? triggerRect.top - gap - renderedHeight : triggerRect.bottom + gap;
  top = clamp(top, viewportTop + margin, viewportTop + viewportHeight - renderedHeight - margin);

  return [
    `top:${Math.round(top)}px`,
    `left:${Math.round(left)}px`,
    `width:${Math.round(width)}px`,
    `max-height:${Math.round(availableHeight)}px`,
    "z-index:var(--z-popover)",
    "opacity:1",
    "pointer-events:auto"
  ].join(";");
}
