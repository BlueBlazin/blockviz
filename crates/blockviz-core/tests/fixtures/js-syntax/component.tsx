import React, { useState, useMemo, type ReactNode } from 'react';
import * as Dialog from '@radix-ui/react-dialog';

type Props<T> = { items: T[]; render: (item: T) => ReactNode; onPick?: (item: T) => void };

export function List<T extends { id: string }>({ items, render, onPick }: Props<T>) {
  const [open, setOpen] = useState(false);
  const sorted = useMemo(() => [...items].sort((a, b) => a.id.localeCompare(b.id)), [items]);
  return (
    <>
      {/* a JSX comment */}
      <Dialog.Root open={open} onOpenChange={setOpen}>
        <Dialog.Trigger asChild><button type="button">Open</button></Dialog.Trigger>
        <svg:rect xlink:href="#a" />
        {sorted.length > 0 ? (
          sorted.map((item) => <Row key={item.id} onClick={() => onPick?.(item)}>{render(item)}</Row>)
        ) : (
          <Empty />
        )}
        {...[<span key="x" />]}
      </Dialog.Root>
    </>
  );
}

const Row = ({ children, ...rest }: { children: ReactNode; onClick: () => void }) => <li {...rest}>{children}</li>;
const Empty = React.memo(function Empty() { return <p>Nothing here</p>; });
export default List;
