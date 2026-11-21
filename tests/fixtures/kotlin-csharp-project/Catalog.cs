namespace Fixture.Catalog
{
    public interface IInventory
    {
        string Find(string sku);
    }

    public enum InventoryState { Ready, Empty }
    public readonly record struct CatalogKey(string Value);

    public sealed class SharpCatalog : IInventory
    {
        public const string DefaultSku = "DEMO", EmptySku = "EMPTY";
        public string Label { get; } = "inventory";

        public SharpCatalog() { }

        public string Find(string sku)
        {
            return CatalogFormatter.Decorate(sku);
        }

        public static SharpCatalog Create() => new SharpCatalog();
    }

    internal static class CatalogFormatter
    {
        public static string Decorate(string sku) => $"item: {sku}";
    }
}
