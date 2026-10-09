import { Ac as defineStd, Il as objectFitProp, Ml as p, yr as videoMedia } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "VideoTexture",
	role: "media",
	species: "custom",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Display a video with customizable playback and object-fit modes",
	acceptsUVContext: true,
	naturalSizeKey: { fromProp: "url" },
	props: {
		url: {
			default: "https://shaders.com/sample.mp4",
			description: "Upload a video or provide a URL",
			ui: {
				type: "video-upload",
				label: "Video",
				group: "Media"
			}
		},
		objectFit: objectFitProp("fill", "How the video should be sized within the viewport", { allowNone: false }),
		loop: {
			default: true,
			description: "Loop the video playback",
			ui: {
				type: "checkbox",
				label: "Loop",
				group: "Playback"
			}
		}
	},
	...videoMedia({
		src: p("url"),
		fit: p("objectFit"),
		loop: p("loop"),
		decode: "srgb-linear-alpha-cut",
		metadataTimeoutMs: 1e4,
		onError: (error, { stage }) => {
			if (stage === "autoplay") console.warn("[VideoTexture] Autoplay failed (browser policy):", error);
			else console.error("[VideoTexture] Failed to load video:", error);
		}
	})
});
var VideoTexture_default = componentDefinition;
export { componentDefinition as n, VideoTexture_default as t };
