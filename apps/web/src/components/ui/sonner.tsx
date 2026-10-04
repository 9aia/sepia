import { Toaster as SonnerToaster, type ToasterProps } from "sonner";

/** Sonner toaster on our tokens — popover surface, ring border, rounded-2xl. */
function Toaster(props: ToasterProps) {
  return (
    <SonnerToaster
      position="bottom-right"
      gap={8}
      toastOptions={{
        classNames: {
          toast:
            "!rounded-2xl !border !border-border !bg-popover !text-popover-foreground !shadow-lg !py-2.5 !px-4 !text-sm",
          title: "!text-sm !font-normal !text-popover-foreground",
          description: "!text-xs !text-muted-foreground",
          actionButton: "!bg-primary !text-primary-foreground !rounded-md",
          cancelButton: "!bg-muted !text-muted-foreground !rounded-md",
          error: "!border-destructive/40",
          success: "!border-border",
        },
      }}
      {...props}
    />
  );
}

export { Toaster };
