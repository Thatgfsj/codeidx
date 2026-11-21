# dart-project（Dart 验收夹具）

codeidx 的 Dart 语言支持验收对象（tree-sitter-dart 0.3.0 驱动）。

## 符号清单（按符号名断言，不按行号）

| 文件 | 符号 | kind |
|---|---|---|
| lib.dart | enum Color | Enum |
| lib.dart | mixin Spinnable（含 spin） | Trait / Method(parent=Spinnable) |
| lib.dart | class VaultApp（含 run） | Class / Method(parent=VaultApp) |
| lib.dart | extension GoldExt on VaultApp（含 forge） | Other / Method(parent=GoldExt) |
| lib.dart | void dart_unique_marker_fn | Fn（引用 dart_helper_fn 3 次） |
| lib.dart | typedef VaultId | Type |
| lib.dart | const kVaultDepth | Const |
| utils.dart | int dart_helper_fn | Fn |

与 sample-project / polyglot-project 互不干扰；本目录是 Dart 支持的黑盒验收对象。
