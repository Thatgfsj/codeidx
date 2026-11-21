package fixture.catalog

const val DefaultSku = "DEMO"
typealias Sku = String

interface Inventory {
    fun find(sku: Sku): String
}

enum class InventoryState { READY, EMPTY }

class KotlinCatalog : Inventory {
    override fun find(sku: Sku): String {
        return CatalogFormatter.decorate(sku)
    }

    companion object {
        fun create(): KotlinCatalog = KotlinCatalog()
    }
}

object CatalogFormatter {
    fun decorate(sku: Sku): String = "item: $sku"
}

fun loadCatalog(): KotlinCatalog = KotlinCatalog.create()
