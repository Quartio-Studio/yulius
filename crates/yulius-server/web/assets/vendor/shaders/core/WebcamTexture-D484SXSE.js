import { Ac as defineStd, Il as objectFitProp, Ml as p, br as webcamMedia } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "WebcamTexture",
	role: "media",
	species: "custom",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Display a live webcam feed with customizable object-fit modes",
	acceptsUVContext: true,
	naturalSizeKey: { fixed: "webcam" },
	props: {
		objectFit: objectFitProp("cover", "How the webcam feed should be sized within the viewport", { allowNone: true }),
		mirror: {
			default: true,
			description: "Mirror the webcam feed horizontally (selfie mode)",
			ui: {
				type: "checkbox",
				label: "Mirror",
				group: "Effect"
			}
		}
	},
	...webcamMedia({
		fit: p("objectFit"),
		mirror: p("mirror"),
		decode: "srgb-linear-alpha-cut",
		constraints: {
			video: {
				width: { ideal: 1280 },
				height: { ideal: 720 },
				facingMode: "user"
			},
			audio: false
		},
		naturalSizeKey: "webcam",
		onError: (error) => {
			if (error instanceof DOMException) if (error.name === "NotAllowedError") console.error("[WebcamTexture] Camera permission denied by user");
			else if (error.name === "NotFoundError") console.error("[WebcamTexture] No camera found on this device");
			else if (error.name === "NotReadableError") console.error("[WebcamTexture] Camera is already in use by another application");
			else console.error("[WebcamTexture] Camera error:", error.message);
			else console.error("[WebcamTexture] Failed to start webcam:", error);
		}
	})
});
var WebcamTexture_default = componentDefinition;
export { componentDefinition as n, WebcamTexture_default as t };
