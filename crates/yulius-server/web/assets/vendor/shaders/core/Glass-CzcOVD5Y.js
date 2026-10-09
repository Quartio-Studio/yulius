import { Ac as defineStd, Cs as recompileWhen, Fl as glassShellProps, Ru as shapeEffectBoundingBoxDeclaration, hi as shapedSurface, jd as createGaussianBlurCompute, yu as glass_exports } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, i as transformBoolean } from "./transformations-P50Uo_Ju.js";
var { applyGlassEffect } = glass_exports;
var DEFAULT_SHAPE_CONFIG = JSON.stringify({
	type: "sphere3D",
	radius: .35
});
function frostBlurCompute(params) {
	const { childNode, gpu, convertToTexture, registerComputeTexture, getCpuValue, onCleanup, onResize, dimensions } = params;
	const root = gpu?.root;
	const blurValue = getCpuValue("blur");
	if (!childNode || !root || typeof blurValue === "number" && blurValue <= 0) return null;
	const childTexture = convertToTexture(childNode);
	let curWidth = Math.max(1, Math.round(dimensions.width));
	let curHeight = Math.max(1, Math.round(dimensions.height));
	const gaussian = createGaussianBlurCompute(root, null, curWidth, curHeight, onCleanup);
	const blurredTexture = registerComputeTexture(gaussian.outputTexture);
	onResize(({ width, height }) => {
		curWidth = Math.max(1, Math.round(width));
		curHeight = Math.max(1, Math.round(height));
		gaussian.setInputDimensions(curWidth, curHeight);
	});
	return {
		outputs: {
			childTexture,
			blurredTexture
		},
		bindInputs: (resolve) => {
			const src = resolve(childTexture.key);
			if (src) gaussian.setInputTexture(src.texture);
		},
		getComputeNodes: () => {
			const b = getCpuValue("blur");
			if (typeof b === "number") gaussian.updateRadius(b * 2);
			return gaussian.computeSteps;
		}
	};
}
var spine = shapedSurface({
	chord: "firstLobe",
	gradSampler: "same",
	bakedGradients: "all",
	stencil: "none",
	child: "required",
	surface: (frame, params) => applyGlassEffect(params, frame.sampler, frame.childTexture, params.computeOutputs?.blurredTexture, {
		volumetric: frame.volumetric,
		bakedGradients: frame.bakedGradients
	})
});
var glassCompute = (params) => {
	const fieldResult = spine.compute(params);
	const blurResult = frostBlurCompute(params);
	if (!fieldResult && !blurResult) return null;
	return {
		outputs: {
			...blurResult?.outputs,
			...fieldResult?.outputs
		},
		bindInputs: blurResult?.bindInputs,
		getComputeNodes: (frameParams) => {
			const nodes = [...blurResult?.getComputeNodes() ?? [], ...fieldResult?.getComputeNodes(frameParams) ?? []];
			return nodes.length ? nodes : null;
		}
	};
};
const componentDefinition = defineStd({
	name: "Glass",
	role: "shapeEffect",
	species: "custom",
	boundingBoxDeclaration: shapeEffectBoundingBoxDeclaration,
	category: "Shape Effects",
	description: "Optically realistic glass lens driven in a custom shape",
	requiresRTT: true,
	requiresChild: true,
	...spine,
	compute: glassCompute,
	props: {
		origin: {
			default: "center",
			description: "Reference edge the center position is measured from (center default). Lets you pin the shape relative to a corner or the canvas centre.",
			ui: {
				type: "origin",
				label: "Origin",
				group: "Position"
			}
		},
		center: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "Center position of the glass shape",
			ui: {
				type: "position",
				label: "Center",
				group: "Position",
				units: ["%", "px"]
			}
		},
		scale: {
			default: 1,
			description: "Scale of the glass shape (1 = default size)",
			ui: {
				type: ["range", "map"],
				min: .1,
				max: 3,
				step: .01,
				label: "Scale",
				group: "Position"
			}
		},
		rotation: {
			default: 0,
			description: "Rotation of the glass shape in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Position"
			}
		},
		cutout: {
			default: false,
			description: "Cut out the alpha outside the glass shape",
			transform: transformBoolean,
			compileTime: true,
			ui: {
				type: "checkbox",
				label: "Cutout",
				group: "Glass"
			}
		},
		...glassShellProps({
			refraction: {
				default: 1,
				description: "Lens refraction — how aggressively the edges warp content beneath (0 = none, 1 = max)",
				ui: { max: 2 }
			},
			edgeSoftness: {
				default: .1,
				description: "Edge softness — higher values give a wider, softer fade at the glass boundary"
			}
		}),
		blur: {
			default: 0,
			recompile: recompileWhen((prev, next) => prev > 0 !== next > 0),
			description: "Frosted blur amount — 0 = clear glass, higher = frosted/diffuse",
			ui: {
				type: "range",
				min: 0,
				max: 20,
				step: .1,
				label: "Blur",
				group: "Glass"
			}
		},
		...glassShellProps({ thickness: {
			transform: (v) => v * .5,
			description: "Glass depth — how far inward from the edge the refraction extends"
		} }),
		aberration: {
			default: .5,
			recompile: recompileWhen((prev, next) => prev > 0 !== next > 0),
			description: "Chromatic aberration — splits RGB channels along the refraction vector",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Aberration",
				group: "Glass"
			}
		},
		innerZoom: {
			default: 1,
			description: "Inner zoom level — magnifies content seen through the glass",
			ui: {
				type: ["range", "map"],
				min: .5,
				max: 3,
				step: .01,
				label: "Inner Zoom",
				group: "Glass"
			}
		},
		...glassShellProps({
			lightAngle: {
				default: 300,
				description: "Light angle in degrees",
				ui: { group: "Highlight" }
			},
			highlight: {
				default: .05,
				description: "Directional edge highlight — bright rim on the light-facing boundary"
			},
			highlightColor: { description: "Color of the directional edge highlight and specular glint" },
			highlightSoftness: { description: "Specular highlight softness" },
			fresnel: { description: "Fresnel rim glow — a soft luminous halo around the glass boundary" },
			fresnelSoftness: true,
			fresnelColor: true
		}),
		tintColor: {
			default: "#ffffff",
			transform: transformColor,
			description: "Color tint applied to the internal directional gradient",
			ui: {
				type: "color",
				label: "Tint Color",
				group: "Tint"
			}
		},
		tintIntensity: {
			default: 0,
			description: "Intensity of the color tint applied to the glass interior",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Tint Intensity",
				group: "Tint"
			}
		},
		tintPreserveLuminosity: {
			default: true,
			transform: transformBoolean,
			description: "Preserve original brightness when tinting",
			ui: {
				type: "checkbox",
				label: "Preserve Luminosity",
				group: "Tint"
			}
		},
		shape: {
			default: DEFAULT_SHAPE_CONFIG,
			description: "Serialized shape configuration (JSON)",
			ui: {
				type: "shape",
				label: "Shape",
				group: "Shape"
			}
		},
		shapeSdfUrl: {
			default: "",
			compileTime: true,
			description: "URL to a pre-generated SDF .bin file — when non-empty, activates SVG mode and triggers a shader recompile"
		},
		shapeType: {
			default: "",
			compileTime: true,
			description: "Active SDF shape type — triggers recompile when shape is switched. When empty, derived from shape JSON at mount time."
		}
	}
});
var Glass_default = componentDefinition;
export { componentDefinition as n, Glass_default as t };
