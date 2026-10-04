"use client";

import { Autocomplete as AutocompletePrimitive } from "@base-ui/react/autocomplete";
import type {
  AutocompleteEmptyProps,
  AutocompleteInputProps,
  AutocompleteItemProps,
  AutocompleteListProps,
  AutocompletePopupProps,
  AutocompletePortalProps,
  AutocompletePositionerProps,
} from "@base-ui/react/autocomplete";
import { cn } from "@/lib/utils";
import { Input } from "./input";
import { ScrollArea } from "./scroll-area";

const Autocomplete = AutocompletePrimitive.Root;

function AutocompleteInput({ className, ...props }: AutocompleteInputProps) {
  // className stays on the primitive — Base UI mergeProps combines it with
  // the render element's, so a state-function className still works.
  return (
    <AutocompletePrimitive.Input
      className={className}
      render={<Input className="h-8 rounded-3xl text-xs" />}
      {...props}
    />
  );
}

function AutocompletePortal(props: AutocompletePortalProps) {
  return <AutocompletePrimitive.Portal {...props} />;
}

function AutocompletePositioner({ className, ...props }: AutocompletePositionerProps) {
  return (
    <AutocompletePrimitive.Positioner sideOffset={4} className={cn("z-50", className)} {...props} />
  );
}

function AutocompletePopup({ className, children, ...props }: AutocompletePopupProps) {
  return (
    <AutocompletePrimitive.Popup
      className={cn(
        "max-h-64 overflow-hidden rounded-xl border border-border bg-popover text-sm text-popover-foreground shadow-lg data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95",
        className,
      )}
      {...props}
    >
      <ScrollArea className="max-h-64" viewportClassName="p-1">
        {children}
      </ScrollArea>
    </AutocompletePrimitive.Popup>
  );
}

function AutocompleteEmpty({ className, ...props }: AutocompleteEmptyProps) {
  return (
    <AutocompletePrimitive.Empty
      className={cn("px-3 py-2 text-xs text-muted-foreground", className)}
      {...props}
    />
  );
}

function AutocompleteList(props: AutocompleteListProps) {
  return <AutocompletePrimitive.List {...props} />;
}

function AutocompleteItem({ className, ...props }: AutocompleteItemProps) {
  return (
    <AutocompletePrimitive.Item
      className={cn(
        "flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-xs outline-hidden data-highlighted:bg-accent data-highlighted:text-accent-foreground",
        className,
      )}
      {...props}
    />
  );
}

export {
  Autocomplete,
  AutocompleteInput,
  AutocompletePortal,
  AutocompletePositioner,
  AutocompletePopup,
  AutocompleteEmpty,
  AutocompleteList,
  AutocompleteItem,
};
