import { Tabs as TabsPrimitive } from "@base-ui/react/tabs";
import { cn } from "cn";

function Tabs({ className, ...props }: TabsPrimitive.Root.Props) {
  return <TabsPrimitive.Root data-slot="tabs" className={cn(className)} {...props} />;
}

function TabsList({ className, ...props }: TabsPrimitive.List.Props) {
  return (
    <TabsPrimitive.List
      data-slot="tabs-list"
      className={cn(
        "inline-flex items-center justify-center rounded-lg bg-muted p-1 text-muted-foreground data-[orientation=vertical]:flex-col data-[orientation=vertical]:items-stretch data-[orientation=vertical]:bg-transparent data-[orientation=vertical]:p-0",
        className,
      )}
      {...props}
    />
  );
}

function TabsTrigger({ className, ...props }: TabsPrimitive.Tab.Props) {
  return (
    <TabsPrimitive.Tab
      data-slot="tabs-trigger"
      className={cn(
        "inline-flex items-center justify-center gap-1.5 rounded-md px-3 py-1.5 text-sm font-medium whitespace-nowrap transition-all focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background",
        "data-[disabled]:pointer-events-none data-[disabled]:opacity-50",
        "data-[active]:bg-secondary data-[active]:text-foreground",
        "data-[orientation=vertical]:justify-start data-[orientation=vertical]:data-[active]:bg-accent",
        className,
      )}
      {...props}
    />
  );
}

function TabsContent({ className, ...props }: TabsPrimitive.Panel.Props) {
  return (
    <TabsPrimitive.Panel data-slot="tabs-content" className={cn("flex-1", className)} {...props} />
  );
}

export { Tabs, TabsList, TabsTrigger, TabsContent };
