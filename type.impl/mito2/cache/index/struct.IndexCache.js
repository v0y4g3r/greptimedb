(function() {
    var type_impls = Object.fromEntries([["mito2",[["<details class=\"toggle implementors-toggle\" open><summary><section id=\"impl-IndexCache%3C(FileId,+u32),+BloomFilterMeta%3E\" class=\"impl\"><a class=\"src rightside\" href=\"src/mito2/cache/index/bloom_filter_index.rs.html#36-48\">source</a><a href=\"#impl-IndexCache%3C(FileId,+u32),+BloomFilterMeta%3E\" class=\"anchor\">§</a><h3 class=\"code-header\">impl <a class=\"struct\" href=\"mito2/cache/index/struct.IndexCache.html\" title=\"struct mito2::cache::index::IndexCache\">IndexCache</a>&lt;(<a class=\"struct\" href=\"mito2/sst/file/struct.FileId.html\" title=\"struct mito2::sst::file::FileId\">FileId</a>, ColumnId), BloomFilterMeta&gt;</h3></section></summary><div class=\"impl-items\"><details class=\"toggle method-toggle\" open><summary><section id=\"method.new\" class=\"method\"><a class=\"src rightside\" href=\"src/mito2/cache/index/bloom_filter_index.rs.html#38-47\">source</a><h4 class=\"code-header\">pub fn <a href=\"mito2/cache/index/struct.IndexCache.html#tymethod.new\" class=\"fn\">new</a>(\n    index_metadata_cap: <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u64.html\">u64</a>,\n    index_content_cap: <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u64.html\">u64</a>,\n    page_size: <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u64.html\">u64</a>,\n) -&gt; Self</h4></section></summary><div class=\"docblock\"><p>Creates a new bloom filter index cache.</p>\n</div></details></div></details>",0,"mito2::cache::index::bloom_filter_index::BloomFilterIndexCache"],["<details class=\"toggle implementors-toggle\" open><summary><section id=\"impl-IndexCache%3CFileId,+InvertedIndexMetas%3E\" class=\"impl\"><a class=\"src rightside\" href=\"src/mito2/cache/index/inverted_index.rs.html#36-48\">source</a><a href=\"#impl-IndexCache%3CFileId,+InvertedIndexMetas%3E\" class=\"anchor\">§</a><h3 class=\"code-header\">impl <a class=\"struct\" href=\"mito2/cache/index/struct.IndexCache.html\" title=\"struct mito2::cache::index::IndexCache\">IndexCache</a>&lt;<a class=\"struct\" href=\"mito2/sst/file/struct.FileId.html\" title=\"struct mito2::sst::file::FileId\">FileId</a>, InvertedIndexMetas&gt;</h3></section></summary><div class=\"impl-items\"><details class=\"toggle method-toggle\" open><summary><section id=\"method.new\" class=\"method\"><a class=\"src rightside\" href=\"src/mito2/cache/index/inverted_index.rs.html#38-47\">source</a><h4 class=\"code-header\">pub fn <a href=\"mito2/cache/index/struct.IndexCache.html#tymethod.new\" class=\"fn\">new</a>(\n    index_metadata_cap: <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u64.html\">u64</a>,\n    index_content_cap: <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u64.html\">u64</a>,\n    page_size: <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u64.html\">u64</a>,\n) -&gt; Self</h4></section></summary><div class=\"docblock\"><p>Creates a new inverted index cache.</p>\n</div></details></div></details>",0,"mito2::cache::index::inverted_index::InvertedIndexCache"],["<details class=\"toggle implementors-toggle\" open><summary><section id=\"impl-IndexCache%3CK,+M%3E\" class=\"impl\"><a class=\"src rightside\" href=\"src/mito2/cache/index.rs.html#133-219\">source</a><a href=\"#impl-IndexCache%3CK,+M%3E\" class=\"anchor\">§</a><h3 class=\"code-header\">impl&lt;K, M&gt; <a class=\"struct\" href=\"mito2/cache/index/struct.IndexCache.html\" title=\"struct mito2::cache::index::IndexCache\">IndexCache</a>&lt;K, M&gt;<div class=\"where\">where\n    K: <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/hash/trait.Hash.html\" title=\"trait core::hash::Hash\">Hash</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/cmp/trait.Eq.html\" title=\"trait core::cmp::Eq\">Eq</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/clone/trait.Clone.html\" title=\"trait core::clone::Clone\">Clone</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/marker/trait.Copy.html\" title=\"trait core::marker::Copy\">Copy</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/marker/trait.Send.html\" title=\"trait core::marker::Send\">Send</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/marker/trait.Sync.html\" title=\"trait core::marker::Sync\">Sync</a> + 'static,\n    M: <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/marker/trait.Send.html\" title=\"trait core::marker::Send\">Send</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/marker/trait.Sync.html\" title=\"trait core::marker::Sync\">Sync</a> + 'static,</div></h3></section></summary><div class=\"impl-items\"><section id=\"method.get_metadata\" class=\"method\"><a class=\"src rightside\" href=\"src/mito2/cache/index.rs.html#138-140\">source</a><h4 class=\"code-header\">pub fn <a href=\"mito2/cache/index/struct.IndexCache.html#tymethod.get_metadata\" class=\"fn\">get_metadata</a>(&amp;self, key: K) -&gt; <a class=\"enum\" href=\"https://doc.rust-lang.org/nightly/core/option/enum.Option.html\" title=\"enum core::option::Option\">Option</a>&lt;<a class=\"struct\" href=\"https://doc.rust-lang.org/nightly/alloc/sync/struct.Arc.html\" title=\"struct alloc::sync::Arc\">Arc</a>&lt;M&gt;&gt;</h4></section><section id=\"method.put_metadata\" class=\"method\"><a class=\"src rightside\" href=\"src/mito2/cache/index.rs.html#142-147\">source</a><h4 class=\"code-header\">pub fn <a href=\"mito2/cache/index/struct.IndexCache.html#tymethod.put_metadata\" class=\"fn\">put_metadata</a>(&amp;self, key: K, metadata: <a class=\"struct\" href=\"https://doc.rust-lang.org/nightly/alloc/sync/struct.Arc.html\" title=\"struct alloc::sync::Arc\">Arc</a>&lt;M&gt;)</h4></section><details class=\"toggle method-toggle\" open><summary><section id=\"method.get_or_load\" class=\"method\"><a class=\"src rightside\" href=\"src/mito2/cache/index.rs.html#151-207\">source</a><h4 class=\"code-header\">async fn <a href=\"mito2/cache/index/struct.IndexCache.html#tymethod.get_or_load\" class=\"fn\">get_or_load</a>&lt;F, Fut, E&gt;(\n    &amp;self,\n    key: K,\n    file_size: <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u64.html\">u64</a>,\n    offset: <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u64.html\">u64</a>,\n    size: <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u32.html\">u32</a>,\n    load: F,\n) -&gt; <a class=\"enum\" href=\"https://doc.rust-lang.org/nightly/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;<a class=\"struct\" href=\"https://doc.rust-lang.org/nightly/alloc/vec/struct.Vec.html\" title=\"struct alloc::vec::Vec\">Vec</a>&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u8.html\">u8</a>&gt;, E&gt;<div class=\"where\">where\n    F: <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/ops/function/trait.Fn.html\" title=\"trait core::ops::function::Fn\">Fn</a>(<a class=\"struct\" href=\"https://doc.rust-lang.org/nightly/alloc/vec/struct.Vec.html\" title=\"struct alloc::vec::Vec\">Vec</a>&lt;<a class=\"struct\" href=\"https://doc.rust-lang.org/nightly/core/ops/range/struct.Range.html\" title=\"struct core::ops::range::Range\">Range</a>&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u64.html\">u64</a>&gt;&gt;) -&gt; Fut,\n    Fut: <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/future/future/trait.Future.html\" title=\"trait core::future::future::Future\">Future</a>&lt;Output = <a class=\"enum\" href=\"https://doc.rust-lang.org/nightly/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;<a class=\"struct\" href=\"https://doc.rust-lang.org/nightly/alloc/vec/struct.Vec.html\" title=\"struct alloc::vec::Vec\">Vec</a>&lt;Bytes&gt;, E&gt;&gt;,\n    E: <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/error/trait.Error.html\" title=\"trait core::error::Error\">Error</a>,</div></h4></section></summary><div class=\"docblock\"><p>Gets given range of index data from cache, and loads from source if the file\nis not already cached.</p>\n</div></details><section id=\"method.get_page\" class=\"method\"><a class=\"src rightside\" href=\"src/mito2/cache/index.rs.html#209-211\">source</a><h4 class=\"code-header\">fn <a href=\"mito2/cache/index/struct.IndexCache.html#tymethod.get_page\" class=\"fn\">get_page</a>(&amp;self, key: K, page_key: <a class=\"struct\" href=\"mito2/cache/index/struct.PageKey.html\" title=\"struct mito2::cache::index::PageKey\">PageKey</a>) -&gt; <a class=\"enum\" href=\"https://doc.rust-lang.org/nightly/core/option/enum.Option.html\" title=\"enum core::option::Option\">Option</a>&lt;Bytes&gt;</h4></section><section id=\"method.put_page\" class=\"method\"><a class=\"src rightside\" href=\"src/mito2/cache/index.rs.html#213-218\">source</a><h4 class=\"code-header\">fn <a href=\"mito2/cache/index/struct.IndexCache.html#tymethod.put_page\" class=\"fn\">put_page</a>(&amp;self, key: K, page_key: <a class=\"struct\" href=\"mito2/cache/index/struct.PageKey.html\" title=\"struct mito2::cache::index::PageKey\">PageKey</a>, value: Bytes)</h4></section></div></details>",0,"mito2::cache::index::bloom_filter_index::BloomFilterIndexCache","mito2::cache::index::inverted_index::InvertedIndexCache"],["<details class=\"toggle implementors-toggle\" open><summary><section id=\"impl-IndexCache%3CK,+M%3E\" class=\"impl\"><a class=\"src rightside\" href=\"src/mito2/cache/index.rs.html#89-131\">source</a><a href=\"#impl-IndexCache%3CK,+M%3E\" class=\"anchor\">§</a><h3 class=\"code-header\">impl&lt;K, M&gt; <a class=\"struct\" href=\"mito2/cache/index/struct.IndexCache.html\" title=\"struct mito2::cache::index::IndexCache\">IndexCache</a>&lt;K, M&gt;<div class=\"where\">where\n    K: <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/hash/trait.Hash.html\" title=\"trait core::hash::Hash\">Hash</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/cmp/trait.Eq.html\" title=\"trait core::cmp::Eq\">Eq</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/marker/trait.Send.html\" title=\"trait core::marker::Send\">Send</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/marker/trait.Sync.html\" title=\"trait core::marker::Sync\">Sync</a> + 'static,\n    M: <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/marker/trait.Send.html\" title=\"trait core::marker::Send\">Send</a> + <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/marker/trait.Sync.html\" title=\"trait core::marker::Sync\">Sync</a> + 'static,</div></h3></section></summary><div class=\"impl-items\"><section id=\"method.new_with_weighter\" class=\"method\"><a class=\"src rightside\" href=\"src/mito2/cache/index.rs.html#94-130\">source</a><h4 class=\"code-header\">pub fn <a href=\"mito2/cache/index/struct.IndexCache.html#tymethod.new_with_weighter\" class=\"fn\">new_with_weighter</a>(\n    index_metadata_cap: <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u64.html\">u64</a>,\n    index_content_cap: <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u64.html\">u64</a>,\n    page_size: <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u64.html\">u64</a>,\n    index_type: &amp;'static <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.str.html\">str</a>,\n    weight_of_metadata: <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.fn.html\">fn</a>(_: <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.reference.html\">&amp;K</a>, _: &amp;<a class=\"struct\" href=\"https://doc.rust-lang.org/nightly/alloc/sync/struct.Arc.html\" title=\"struct alloc::sync::Arc\">Arc</a>&lt;M&gt;) -&gt; <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u32.html\">u32</a>,\n    weight_of_content: <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.fn.html\">fn</a>(_: &amp;(K, <a class=\"struct\" href=\"mito2/cache/index/struct.PageKey.html\" title=\"struct mito2::cache::index::PageKey\">PageKey</a>), _: &amp;Bytes) -&gt; <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u32.html\">u32</a>,\n) -&gt; Self</h4></section></div></details>",0,"mito2::cache::index::bloom_filter_index::BloomFilterIndexCache","mito2::cache::index::inverted_index::InvertedIndexCache"]]]]);
    if (window.register_type_impls) {
        window.register_type_impls(type_impls);
    } else {
        window.pending_type_impls = type_impls;
    }
})()
//{"start":55,"fragment_lengths":[12691]}