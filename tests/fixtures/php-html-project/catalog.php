<?php

namespace Demo\Catalog {
    const DEFAULT_LABEL = 'catalog', FALLBACK_LABEL = 'unknown';
    interface Labelled {
        public function label(): string;
    }
    trait FormatsLabels {
        public function formatLabel(string $label): string {
            return strtoupper($label);
        }
    }
    enum Stock: string {
        case Ready = 'ready';
    }
    class Product implements Labelled {
        use FormatsLabels;
        public const MAX_ITEMS = 10;
        private string $label = DEFAULT_LABEL;
        public function label(): string {
            return $this->formatLabel($this->label);
        }
    }
    function makeProduct(): Product {
        return new Product();
    }
    function describeProduct(): string {
        $product = \Demo\Catalog\makeProduct();
        return $product?->label() ?? FALLBACK_LABEL;
    }
    $anonymous = function () {};
}
?>
<footer>End of template</footer>
