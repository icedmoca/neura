import { useCallback, useEffect, useRef, useState } from "react";
import { useDapp } from "./DappProvider";

type LayerContent = string | null;

export default function DappPreview() {
  const { previewSrc } = useDapp();
  const [frontLayer, setFrontLayer] = useState<0 | 1>(0);
  const [layers, setLayers] = useState<[LayerContent, LayerContent]>([null, null]);
  const frontLayerRef = useRef<0 | 1>(0);
  const displayedSrcRef = useRef<string | null>(null);
  const inflightRef = useRef(0);
  const pendingFrontRef = useRef<0 | 1 | null>(null);
  const layerSrcRef = useRef<[string | null, string | null]>([null, null]);
  const previewRootRef = useRef(previewSrc.split("?")[0]);

  const promoteLayer = useCallback((layer: 0 | 1, src: string) => {
    pendingFrontRef.current = null;
    frontLayerRef.current = layer;
    setFrontLayer(layer);
    displayedSrcRef.current = src;
  }, []);

  const handleLoad = useCallback((layer: 0 | 1) => {
    if (pendingFrontRef.current !== layer) return;
    const src = layerSrcRef.current[layer];
    if (!src) return;

    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        if (pendingFrontRef.current !== layer) return;
        promoteLayer(layer, src);
      });
    });
  }, [promoteLayer]);

  const syncPreview = useCallback(async (src: string) => {
    if (src === displayedSrcRef.current) return;

    const ticket = ++inflightRef.current;
    try {
      const res = await fetch(src, { cache: "no-store" });
      if (!res.ok) return;
      const html = await res.text();
      if (ticket !== inflightRef.current) return;

      const backLayer: 0 | 1 = frontLayerRef.current === 0 ? 1 : 0;
      layerSrcRef.current[backLayer] = src;
      pendingFrontRef.current = backLayer;

      setLayers((prev) => {
        const next: [LayerContent, LayerContent] = [...prev];
        next[backLayer] = html;
        return next;
      });
    } catch {
      pendingFrontRef.current = null;
    }
  }, []);

  useEffect(() => {
    const nextRoot = previewSrc.split("?")[0];
    if (nextRoot !== previewRootRef.current) {
      previewRootRef.current = nextRoot;
      displayedSrcRef.current = null;
      inflightRef.current += 1;
      pendingFrontRef.current = null;
      frontLayerRef.current = 0;
      setFrontLayer(0);
      setLayers([null, null]);
      layerSrcRef.current = [null, null];
    }
    void syncPreview(previewSrc);
  }, [previewSrc, syncPreview]);

  return (
    <div className="dapp-preview" aria-hidden={false}>
      {([0, 1] as const).map((layer) => {
        const html = layers[layer];
        if (!html) return null;

        const isFront = frontLayer === layer;
        return (
          <iframe
            key={layer}
            className={`dapp-preview__frame${isFront ? " dapp-preview__frame--front" : ""}`}
            title="Project dapp preview"
            srcDoc={html}
            onLoad={() => handleLoad(layer)}
            sandbox="allow-scripts allow-forms allow-popups"
          />
        );
      })}
    </div>
  );
}
