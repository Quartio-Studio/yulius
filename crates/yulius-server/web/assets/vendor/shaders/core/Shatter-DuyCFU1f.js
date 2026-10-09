import { Ac as defineStd, Bd as ZERO, Fd as applyEdgeHandlingExpr, Hd as call, Kd as floatE, _t as crackRefractUV, gt as crackGeom, ht as cellLaneUVFor, id as toHalfFloat, if as unpremultiplyAlpha, vt as crackShardCompose, yt as voronoiRegionField } from "./std-ClftfWpF.js";
import { s as transformEdges } from "./transformations-P50Uo_Ju.js";
var seededRandom = (seed) => {
	const x = Math.sin(seed) * 1e4;
	return x - Math.floor(x);
};
var SHARD_COUNT = 16;
var VORONOI_SIZE = 1024;
var DATA_WIDTH = SHARD_COUNT * 4;
var cellLaneUV = cellLaneUVFor(DATA_WIDTH);
var shardSites = (seed) => {
	const positions = new Float32Array(SHARD_COUNT * 2);
	for (let i = 0; i < SHARD_COUNT; i++) {
		positions[i * 2] = seededRandom(seed + i * 2);
		positions[i * 2 + 1] = seededRandom(seed + i * 2 + 1);
	}
	return positions;
};
function shardPhysics(getCpuValue) {
	const cellData = new Float32Array(SHARD_COUNT * 4);
	const displacementData = new Float32Array(SHARD_COUNT * 4);
	const generateCells = (seed) => {
		const sites = shardSites(seed);
		for (let i = 0; i < SHARD_COUNT; i++) {
			cellData[i * 4] = sites[i * 2];
			cellData[i * 4 + 1] = sites[i * 2 + 1];
			cellData[i * 4 + 2] = seededRandom(seed + i * 3);
			cellData[i * 4 + 3] = seededRandom(seed + i * 3 + 1);
		}
	};
	let currentSeed = getCpuValue("seed") ?? 2;
	generateCells(currentSeed);
	const FRICTION = 1;
	let prevX = .5;
	let prevY = .5;
	return {
		cellData,
		displacementData,
		step(pointer, deltaTime, aspect) {
			const dt = Math.min(deltaTime, .016);
			const newSeed = getCpuValue("seed") ?? 2;
			if (newSeed !== currentSeed) {
				currentSeed = newSeed;
				generateCells(currentSeed);
				displacementData.fill(0);
			}
			const intensity = getCpuValue("intensity") ?? 4;
			const radius = getCpuValue("radius") ?? .4;
			const decay = getCpuValue("decay") ?? 1;
			let velX = dt > 0 ? (pointer.x - prevX) / dt : 0;
			let velY = dt > 0 ? (pointer.y - prevY) / dt : 0;
			let speed = Math.sqrt(velX * velX + velY * velY);
			const maxVelocity = 5 + intensity * 2;
			if (speed > maxVelocity) {
				const scale = maxVelocity / speed;
				velX *= scale;
				velY *= scale;
				speed = maxVelocity;
			}
			for (let i = 0; i < SHARD_COUNT; i++) {
				const cellX = cellData[i * 4];
				const cellY = cellData[i * 4 + 1];
				const randomDirX = cellData[i * 4 + 2] - .5;
				const randomDirY = cellData[i * 4 + 3] - .5;
				const dx = aspect >= 1 ? (cellX - pointer.x) * aspect : cellX - pointer.x;
				const dy = aspect >= 1 ? cellY - pointer.y : (cellY - pointer.y) / aspect;
				const dist = Math.sqrt(dx * dx + dy * dy);
				let currentDx = displacementData[i * 4];
				let currentDy = displacementData[i * 4 + 1];
				const decayTime = Math.max(.01, decay);
				const decayFactor = Math.exp(-dt / decayTime);
				currentDx *= decayFactor;
				currentDy *= decayFactor;
				let velocityDx = 0;
				let velocityDy = 0;
				if (dist < radius && speed > .01) {
					const influence = Math.max(0, 1 - dist / radius);
					const influenceCurve = influence * influence;
					const pushForce = influenceCurve * speed * intensity * dt * .5;
					velocityDx = velX * pushForce;
					velocityDy = velY * pushForce;
					const jitterForce = influenceCurve * speed * intensity * dt * .1;
					velocityDx += randomDirX * jitterForce;
					velocityDy += randomDirY * jitterForce;
				}
				const lerpFactor = Math.min(1, FRICTION * dt);
				displacementData[i * 4] = currentDx + velocityDx * lerpFactor;
				displacementData[i * 4 + 1] = currentDy + velocityDy * lerpFactor;
			}
			prevX = pointer.x;
			prevY = pointer.y;
		}
	};
}
function shardDataTexture(params, physics) {
	const { createDataTexture, registerMediaTexture, onCleanup } = params;
	const dataMirror = new Float32Array(DATA_WIDTH);
	const texData = new Uint16Array(DATA_WIDTH);
	const packMirror = () => {
		for (let i = 0; i < SHARD_COUNT; i++) {
			dataMirror[i * 4] = physics.cellData[i * 4];
			dataMirror[i * 4 + 1] = physics.cellData[i * 4 + 1];
			dataMirror[i * 4 + 2] = physics.displacementData[i * 4];
			dataMirror[i * 4 + 3] = physics.displacementData[i * 4 + 1];
		}
		for (let i = 0; i < DATA_WIDTH; i++) texData[i] = toHalfFloat(dataMirror[i]);
	};
	packMirror();
	const dataTex = createDataTexture({
		width: DATA_WIDTH,
		height: 1,
		format: "r16float",
		data: texData,
		label: "shatter-cells"
	});
	onCleanup(() => dataTex.destroy());
	return {
		dataKit: registerMediaTexture(() => dataTex.texture),
		upload() {
			packMirror();
			dataTex.write(texData);
		}
	};
}
const componentDefinition = defineStd({
	name: "Shatter",
	role: "simulation",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Interactive",
	description: "Broken glass effect with tectonic plate displacement",
	requiresRTT: true,
	requiresChild: true,
	usesPointer: true,
	props: {
		crackWidth: {
			default: 1,
			description: "Thickness of crack lines",
			ui: {
				type: ["range", "map"],
				min: .5,
				max: 5,
				step: .1,
				label: "Crack Width",
				group: "Effect"
			}
		},
		intensity: {
			default: 4,
			description: "How much shards shift",
			ui: {
				type: "range",
				min: 0,
				max: 20,
				step: 1,
				label: "Intensity",
				group: "Effect"
			}
		},
		radius: {
			default: .4,
			description: "Cursor influence radius",
			ui: {
				type: "range",
				min: .1,
				max: 1,
				step: .1,
				label: "Radius",
				group: "Effect"
			}
		},
		decay: {
			default: 1,
			description: "How fast shards return to rest",
			ui: {
				type: "range",
				min: .1,
				max: 10,
				step: .1,
				label: "Decay",
				group: "Effect"
			}
		},
		seed: {
			default: 2,
			description: "Random seed for pattern",
			ui: {
				type: "range",
				min: 0,
				max: 50,
				step: 1,
				label: "Seed",
				group: "Effect"
			}
		},
		chromaticSplit: {
			default: 1,
			description: "RGB separation for prismatic glass effect",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 5,
				step: .1,
				label: "Chromatic Split",
				group: "Effect"
			}
		},
		refractionStrength: {
			default: 5,
			description: "How much cracks bend/distort the underlying image",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 10,
				step: .1,
				label: "Refraction",
				group: "Effect"
			}
		},
		shardLighting: {
			default: .1,
			description: "Subtle lighting on tilted shards for 3D depth",
			ui: {
				type: "range",
				min: 0,
				max: .5,
				step: .1,
				label: "Shard Lighting",
				group: "Effect"
			}
		},
		edges: {
			default: "mirror",
			description: "How to handle edges when displacement pushes content out of bounds",
			transform: transformEdges,
			compileTime: true,
			ui: {
				type: "select",
				options: [
					{
						label: "Stretch",
						value: "stretch"
					},
					{
						label: "Transparent",
						value: "transparent"
					},
					{
						label: "Mirror",
						value: "mirror"
					},
					{
						label: "Wrap",
						value: "wrap"
					}
				],
				label: "Edges",
				group: "Effect"
			}
		}
	},
	compute: voronoiRegionField({
		count: SHARD_COUNT,
		size: VORONOI_SIZE,
		sites: shardSites,
		seedProp: "seed",
		seedDefault: 2,
		output: "voronoiField"
	}),
	gpu: { fragment: (params) => {
		const { childNode, ctx, computeOutputs, propValues, convertToTexture } = params;
		if (!childNode) return ZERO;
		const child = convertToTexture(childNode);
		const voronoiField = computeOutputs?.voronoiField;
		if (!voronoiField) return call(unpremultiplyAlpha, "unpremultiplyAlpha", [child.sample(ctx.uv)]);
		const { onBeforeRender, getCpuValue, dimensions } = params;
		const edgeMode = propValues.edges ?? 0;
		const physics = shardPhysics(getCpuValue);
		const cells = shardDataTexture(params, physics);
		const dataKit = cells.dataKit;
		onBeforeRender((fp) => {
			const { pointer, deltaTime } = fp;
			const aspect = dimensions.width / Math.max(1, dimensions.height);
			physics.step(pointer, deltaTime, aspect);
			cells.upload();
		});
		const vSample = voronoiField.sample(ctx.uv, "nearestClamp");
		const nearestI = vSample.member("r");
		const secondI = vSample.member("g");
		const nearestClamp = "nearestClamp";
		const dataAt = (idx, field) => dataKit.sample(call(cellLaneUV, "cellLaneUV", [idx, floatE(field)]), nearestClamp).member("r");
		const geom = call(crackGeom, "crackGeom", [
			ctx.uv,
			dataAt(nearestI, 0),
			dataAt(nearestI, 1),
			dataAt(secondI, 0),
			dataAt(secondI, 1),
			dataAt(nearestI, 2),
			dataAt(nearestI, 3),
			params.uniforms.crackWidth
		]);
		const crackIntensity = geom.member("crackIntensity");
		const edgeNormal = geom.member("edgeNormal");
		const displacedUV = geom.member("displacedUV");
		const disp = geom.member("disp");
		const sample = (uv) => applyEdgeHandlingExpr(uv, (u) => child.sample(u), edgeMode);
		const normalColor = sample(displacedUV);
		const refractUV = (channel) => call(crackRefractUV, "crackRefractUV", [
			displacedUV,
			edgeNormal,
			crackIntensity,
			params.uniforms.refractionStrength,
			params.uniforms.chromaticSplit,
			floatE(channel)
		]);
		return call(unpremultiplyAlpha, "unpremultiplyAlpha", [call(crackShardCompose, "crackShardCompose", [
			normalColor,
			sample(refractUV(1)),
			sample(refractUV(0)),
			sample(refractUV(-1)),
			crackIntensity,
			disp,
			params.uniforms.shardLighting
		])]);
	} }
});
var Shatter_default = componentDefinition;
export { componentDefinition as n, Shatter_default as t };
