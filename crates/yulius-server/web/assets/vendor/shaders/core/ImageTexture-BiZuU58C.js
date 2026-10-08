import { Ac as defineStd, Il as objectFitProp, Ml as p, _r as imageMedia } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "ImageTexture",
	role: "media",
	species: "custom",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Display an image with customizable object-fit modes",
	acceptsUVContext: true,
	naturalSizeKey: { fromProp: "url" },
	props: {
		url: {
			default: "https://shaders.com/sample.jpg",
			description: "Upload an image or provide a URL",
			ui: {
				type: "image-upload",
				label: "Image",
				group: "Media"
			}
		},
		objectFit: objectFitProp("fill", "How the image should be sized within the viewport", { allowNone: false })
	},
	...imageMedia({
		src: p("url"),
		fit: p("objectFit"),
		decode: "srgb-linear-alpha-cut",
		label: "ImageTexture",
		onError(error, url) {
			const isRelative = !url.startsWith("http://") && !url.startsWith("https://") && !url.startsWith("data:");
			console.error(`[ImageTexture] Failed to load image\n  Requested URL: ${url}\n  Path type: ${isRelative ? "relative" : "absolute"}\n  Error:`, error);
		}
	})
});
var ImageTexture_default = componentDefinition;
export { componentDefinition as n, ImageTexture_default as t };
