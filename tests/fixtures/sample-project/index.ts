// Sample TypeScript fixture: entry module.

interface StoreItem {
  id: number;
}

class RegistryStore {
  items: StoreItem[] = [];

  add(item: StoreItem): void {
    this.items.push(item);
  }
}

function dispatch(store: RegistryStore, id: number): void {
  store.add({ id });
}
