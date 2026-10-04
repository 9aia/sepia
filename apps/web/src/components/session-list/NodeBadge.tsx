import { useMultiNode, useNodeLabel } from "../../hooks/query/useNodes";
import { Badge } from "../ui/badge";

/**
 * Machine tag on a session or project row (`name` = the node's hostname).
 * Renders nothing while no peers are registered — single-node lists stay
 * visually identical to before (docs/protocol.md phase 1).
 */
export function NodeBadge({ node }: { readonly node?: string }) {
  const multi = useMultiNode();
  const label = useNodeLabel(node);
  if (!multi) return null;
  return (
    <Badge variant="secondary" className="shrink-0 font-normal" title={`On ${label}`}>
      {label}
    </Badge>
  );
}
